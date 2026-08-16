//! Read-only access to ZCode's on-disk session state.
//!
//! ZCode stores its live agent state in a SQLite database at
//! `~/.zcode/cli/db/db.sqlite`.  The daemon periodically polls this database
//! to mirror active ZCode sessions on the Stream Deck task board, the same
//! way Codex Micro activity arrives via `v.oai.thstatus` serial frames.
//!
//! `model_usage` rows are only written when a model request *finishes*, so
//! the latest row always carries a terminal status (`completed`,
//! `cancelled`, or `error`) and cannot distinguish an in-flight turn from a
//! finished one.  Live activity is instead detected from the session's
//! newest persisted message: a user message (a fresh prompt or tool result
//! awaiting its next model call), an assistant message still streaming
//! (`time.created` persisted without `time.completed`), or an assistant
//! message that ended in tool calls all mean the turn is still running.  A
//! recency bound on `session.time_updated` keeps aborted turns, whose
//! streaming message is never finalized, from looking active forever.
//!
//! All queries open the database read-only so the running ZCode process keeps
//! its write lock undisturbed.  If the database is missing, locked, or has an
//! unexpected schema, [`read_zcode_snapshot`] returns `None` so the caller can
//! leave the existing board untouched rather than blanking it.

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

use crate::tasks::{TASK_PRIORITY_ACTIVE, TASK_PRIORITY_IDLE};

/// How recently a session must have been updated for a structurally open
/// turn to still count as running.  Tool executions between model calls can
/// pause database writes for minutes; past this bound an open tail is
/// treated as an abandoned or aborted turn.
const RUNNING_WINDOW_MS: u128 = 300_000;

/// Returns the path to the ZCode CLI database, or `None` when the home
/// directory cannot be resolved.
pub fn zcode_db_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    Some(
        std::path::Path::new(&home)
            .join(".zcode")
            .join("cli")
            .join("db")
            .join("db.sqlite"),
    )
}

/// Reads the live ZCode session snapshot and returns it as a
/// `{"tasks": [...]}` payload ready for [`crate::tasks::TaskBoard::publish_tasks`].
///
/// Returns `None` when the database is unavailable (missing, locked, or
/// unreadable) so the caller can preserve the previously published board.
pub fn read_zcode_snapshot(now_ms: u128, max_tasks: usize) -> Option<Value> {
    let path = zcode_db_path()?;
    let connection = Connection::open_with_flags(
        &path,
        // Read-only: never acquire a write lock on ZCode's database.
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    // Tolerate a transient write lock from the running ZCode process.
    connection
        .busy_timeout(std::time::Duration::from_millis(100))
        .ok()?;

    read_zcode_snapshot_from_connection(&connection, now_ms, max_tasks)
}

fn read_zcode_snapshot_from_connection(
    connection: &Connection,
    now_ms: u128,
    max_tasks: usize,
) -> Option<Value> {
    // LEFT JOIN: a brand-new session has no finished model request yet, but
    // its persisted prompt already makes it a live, running task.
    let mut statement = connection
        .prepare(
            "SELECT s.id, s.title, s.directory, s.time_updated,
                    mu.status, mu.model_id, mu.variant, mu.started_at, mu.completed_at
             FROM session AS s
             LEFT JOIN model_usage AS mu ON mu.rowid = (
                 SELECT latest.rowid FROM model_usage AS latest
                 WHERE latest.session_id = s.id
                 ORDER BY latest.started_at DESC, latest.rowid DESC LIMIT 1
             )
             WHERE s.time_archived IS NULL
               AND s.task_type IN ('interactive', 'task')
             ORDER BY CASE WHEN mu.status = 'running' THEN 0 ELSE 1 END,
                      MAX(s.time_updated, COALESCE(mu.started_at, 0), COALESCE(mu.completed_at, 0)) DESC,
                      s.id ASC
             LIMIT ?1;",
        )
        .ok()?;

    let rows = statement
        .query_map([i64::try_from(max_tasks).unwrap_or(i64::MAX)], |row| {
            let status: Option<String> = row.get(4)?;
            let model_id = row.get(5)?;
            let variant = row.get(6)?;
            let started_at = row.get::<_, Option<i64>>(7)?.unwrap_or(0);
            let completed_at = row.get(8)?;
            let latest = status.map(|status| ModelUsage {
                status,
                model_id,
                variant,
                started_at,
                completed_at,
            });
            Ok(SessionRow {
                id: row.get(0)?,
                title: row.get(1)?,
                directory: row.get(2)?,
                time_updated: row.get(3)?,
                latest,
            })
        })
        .ok()?;

    let mut tasks = Vec::new();
    for row in rows.flatten() {
        tasks.push(build_task(&connection, &row, now_ms));
    }

    if tasks.is_empty() {
        // Distinguish "DB readable but nothing active" from "DB missing".
        // An empty array is a valid publish: it clears stale ZCode cards.
        return Some(json!({"tasks": []}));
    }
    Some(json!({"tasks": tasks}))
}

struct SessionRow {
    id: String,
    title: String,
    directory: String,
    time_updated: i64,
    latest: Option<ModelUsage>,
}
/// Builds a single task object for one session, enriching it with the latest
/// model-usage status and the model/effort selection.
fn build_task(connection: &Connection, row: &SessionRow, now_ms: u128) -> Value {
    let selection = model_selection(connection, &row.id);
    let tail = session_tail(connection, &row.id);
    let latest = row.latest.as_ref();

    let state = derive_state(latest, tail.as_ref(), row.time_updated, now_ms);
    let started_at_ms = latest.map(|usage| usage.started_at);
    let finished_at_ms = latest.and_then(|usage| usage.completed_at);
    let model = selection
        .as_ref()
        .and_then(|s| s.model.clone())
        .or_else(|| latest.and_then(|usage| usage.model_id.clone()));
    let effort = selection
        .as_ref()
        .and_then(|s| s.thought_level.clone())
        .or_else(|| latest.and_then(|usage| usage.variant.clone()));
    let project = project_name(&row.directory);

    // Running sessions rank above idle ones so they claim the lowest slots.
    let priority = if state == "running" {
        TASK_PRIORITY_ACTIVE
    } else {
        TASK_PRIORITY_IDLE
    };

    let context = json!({
        "project": project,
        "workspace_path": row.directory,
        "task": row.title,
        "model": model,
        "effort": effort,
        "status": state,
        "task_id": format!("zcode:{}", row.id),
    });

    json!({
        "task_id": format!("zcode:{}", row.id),
        "title": row.title,
        "workspace_path": row.directory,
        "state": state,
        "priority": priority,
        // A running turn without a finished request yet has no request start
        // timestamp; the session's last update is the closest stable anchor.
        "started_at_ms": (state != "queued").then_some(started_at_ms.or(Some(row.time_updated))),
        "finished_at_ms": (state == "completed").then_some(
            finished_at_ms.unwrap_or_else(|| started_at_ms.unwrap_or(0).max(row.time_updated))
        ),
        "timing_authoritative": true,
        "context": context,
    })
}

/// The most recent `model_usage` row for a session, if any.
struct ModelUsage {
    status: String,
    model_id: Option<String>,
    variant: Option<String>,
    started_at: i64,
    completed_at: Option<i64>,
}

/// Lifecycle shape of a session's newest persisted message.
struct SessionTail {
    role: String,
    /// Assistant message whose completion timestamp has not been written yet.
    streaming: bool,
    /// Assistant message that finished with tool calls still executing.
    ends_with_tool_calls: bool,
}

/// Reads the newest message of a session and reduces it to its lifecycle
/// shape.  Returns `None` when the message table is unavailable or the
/// session has no messages, so state derivation falls back to the terminal
/// model-usage status.
fn session_tail(connection: &Connection, session_id: &str) -> Option<SessionTail> {
    // Two indexed point queries rather than one ordered scan: sorting by the
    // sequence tiebreak would make SQLite read every message blob of the
    // session, and long sessions carry hundreds of them.
    let latest_time: i64 = connection
        .prepare("SELECT MAX(time_created) FROM message WHERE session_id = ?1")
        .ok()?
        .query_row([session_id], |row| row.get(0))
        .ok()?;
    let data: String = connection
        .prepare(
            "SELECT data FROM message
             WHERE session_id = ?1 AND time_created = ?2
             ORDER BY sequence DESC LIMIT 1",
        )
        .ok()?
        .query_row(rusqlite::params![session_id, latest_time], |row| row.get(0))
        .ok()?;
    let value: Value = serde_json::from_str(&data).ok()?;
    let role = value.get("role").and_then(Value::as_str)?.to_owned();
    if role != "user" && role != "assistant" {
        return None;
    }
    let completed = value
        .get("time")
        .and_then(|time| time.get("completed"))
        .and_then(Value::as_i64)
        .filter(|completed| *completed > 0);
    let is_assistant = role == "assistant";
    Some(SessionTail {
        streaming: is_assistant && completed.is_none(),
        ends_with_tool_calls: value.get("finish").and_then(Value::as_str) == Some("tool-calls"),
        role,
    })
}

struct ModelSelection {
    model: Option<String>,
    thought_level: Option<String>,
}

fn model_selection(connection: &Connection, session_id: &str) -> Option<ModelSelection> {
    let mut statement = connection
        .prepare(
            "SELECT data
             FROM session_entry
             WHERE session_id = ?1 AND type = 'runtime/model_selection'
             ORDER BY time_created DESC
             LIMIT 1;",
        )
        .ok()?;
    let data: String = statement.query_row([session_id], |row| row.get(0)).ok()?;
    let value: Value = serde_json::from_str(&data).ok()?;
    Some(ModelSelection {
        model: value
            .get("modelId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        thought_level: value
            .get("thoughtLevel")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Maps the terminal model-usage status and live message tail to a
/// task-board state.  A structurally open, recently active turn is running
/// regardless of the previous request's terminal status.  Completion remains
/// visible until the task board acknowledges it through selection.
fn derive_state(
    latest: Option<&ModelUsage>,
    tail: Option<&SessionTail>,
    session_updated_ms: i64,
    now_ms: u128,
) -> &'static str {
    if turn_in_flight(tail, session_updated_ms, now_ms) {
        return "running";
    }
    match latest {
        // No finished request: the session exists but has not produced a
        // terminal status yet.
        None => "queued",
        Some(latest) => match latest.status.trim().to_ascii_lowercase().as_str() {
            "running" => "running",
            "error" => "error",
            "cancelled" => "queued",
            "completed" => "completed",
            _ => "error",
        },
    }
}

/// True when the session's newest message shows a turn still in flight and
/// the session was updated recently enough for that to be trustworthy.
fn turn_in_flight(tail: Option<&SessionTail>, session_updated_ms: i64, now_ms: u128) -> bool {
    let Some(tail) = tail else { return false };
    let structurally_open = match tail.role.as_str() {
        // A user message is a fresh prompt or a tool result awaiting its
        // next model call.
        "user" => true,
        "assistant" => tail.streaming || tail.ends_with_tool_calls,
        _ => false,
    };
    if !structurally_open {
        return false;
    }
    let last_update_ms = u128::try_from(session_updated_ms.max(0)).unwrap_or(0);
    now_ms.saturating_sub(last_update_ms) < RUNNING_WINDOW_MS
}

/// Reduces a workspace directory to a short project label (the final path
/// segment), mirroring how Codex derives its project name from a session cwd.
fn project_name(directory: &str) -> String {
    std::path::Path::new(directory)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(directory)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Builds an in-memory database with the ZCode schema subset we query, for
    /// deterministic mapping tests that do not touch the real database.
    fn fixture_db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                    id text primary key,
                    project_id text not null,
                    workspace_id text,
                    parent_id text,
                    slug text not null,
                    directory text not null,
                    path text,
                    title text not null,
                    version text not null,
                    time_created integer not null,
                    time_updated integer not null,
                    time_archived integer,
                    task_type text not null default 'interactive'
                );
                CREATE TABLE session_entry (
                    id text primary key,
                    session_id text not null,
                    type text not null,
                    time_created integer not null,
                    time_updated integer not null,
                    data text not null
                );
                CREATE TABLE model_usage (
                    id text primary key,
                    logical_request_id text not null,
                    session_id text not null,
                    provider_id text not null,
                    model_id text not null,
                    variant text,
                    agent text,
                    status text not null,
                    started_at integer not null,
                    completed_at integer
                );
                CREATE TABLE message (
                    id text primary key,
                    session_id text not null,
                    time_created integer not null,
                    time_updated integer not null,
                    sequence integer,
                    data text not null
                );",
            )
            .unwrap();
        connection
    }

    /// Inserts one persisted message, mirroring the lifecycle fields ZCode
    /// writes: `time.completed` and `finish` appear only when generation
    /// finishes.
    fn insert_message(
        connection: &Connection,
        session_id: &str,
        sequence: i64,
        time_created: i64,
        role: &str,
        finish: Option<&str>,
        completed_at: Option<i64>,
    ) {
        let mut time = serde_json::Map::new();
        time.insert("created".to_owned(), json!(time_created));
        if let Some(completed_at) = completed_at {
            time.insert("completed".to_owned(), json!(completed_at));
        }
        let mut message = serde_json::Map::new();
        message.insert("role".to_owned(), json!(role));
        message.insert("time".to_owned(), Value::Object(time));
        if let Some(finish) = finish {
            message.insert("finish".to_owned(), json!(finish));
        }
        connection
            .execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, sequence, data)
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
                rusqlite::params![
                    format!("msg-{session_id}-{sequence}"),
                    session_id,
                    time_created,
                    sequence,
                    Value::Object(message).to_string()
                ],
            )
            .unwrap();
    }

    fn insert_session(connection: &Connection, id: &str, time_updated: i64) {
        connection
            .execute(
                "INSERT INTO session (id, project_id, slug, directory, title, version,
                                      time_created, time_updated, task_type)
                 VALUES (?1, 'p1', 'slug', 'D:\\proj\\micro-emu', ?1, '1', 1000, ?2, 'interactive')",
                rusqlite::params![id, time_updated],
            )
            .unwrap();
    }

    fn insert_usage(
        connection: &Connection,
        id: &str,
        session_id: &str,
        status: &str,
        started_at: i64,
        completed_at: Option<i64>,
    ) {
        connection
            .execute(
                "INSERT INTO model_usage (id, logical_request_id, session_id, provider_id,
                                          model_id, variant, agent, status, started_at, completed_at)
                 VALUES (?1, 'r1', ?2, 'builtin:zai-coding-plan',
                         'GLM-5.3', 'max', 'zcode-agent', ?3, ?4, ?5)",
                rusqlite::params![id, session_id, status, started_at, completed_at],
            )
            .unwrap();
    }

    #[test]
    fn maps_running_session_to_running_task() {
        let connection = fixture_db();
        connection
            .execute(
                "INSERT INTO session (id, project_id, slug, directory, title, version,
                                      time_created, time_updated, task_type)
                 VALUES ('sess_a', 'p1', 'a', 'D:\\proj\\micro-emu', 'Fix task buttons',
                         '1', 1000, 5000, 'interactive')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO model_usage (id, logical_request_id, session_id, provider_id,
                                          model_id, variant, agent, status, started_at)
                 VALUES ('mu1', 'r1', 'sess_a', 'builtin:zai-coding-plan',
                         'GLM-5.2', 'max', 'zcode-agent', 'running', 4900)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session_entry (id, session_id, type, time_created, time_updated, data)
                 VALUES ('se1', 'sess_a', 'runtime/model_selection', 1000, 1000,
                         '{\"modelId\":\"GLM-5.2\",\"thoughtLevel\":\"max\"}')",
                [],
            )
            .unwrap();

        let snapshot = read_zcode_snapshot_from_connection(&connection, 6000, 6).unwrap();
        let task = &snapshot["tasks"][0];
        assert_eq!(task["task_id"], "zcode:sess_a");
        assert_eq!(task["state"], "running");
        assert_eq!(task["priority"], 75);
        assert_eq!(task["context"]["model"], "GLM-5.2");
        assert_eq!(task["context"]["effort"], "max");
        assert_eq!(task["context"]["project"], "micro-emu");
        assert_eq!(task["context"]["task"], "Fix task buttons");
    }

    #[test]
    fn old_completed_session_stays_completed_until_selection() {
        let connection = fixture_db();
        connection
            .execute(
                "INSERT INTO session (id, project_id, slug, directory, title, version,
                                      time_created, time_updated, task_type)
                 VALUES ('sess_b', 'p1', 'b', 'D:\\proj\\other', 'Old task', '1',
                         1000, 1000, 'interactive')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO model_usage (id, logical_request_id, session_id, provider_id,
                                          model_id, variant, agent, status, started_at)
                 VALUES ('mu2', 'r2', 'sess_b', 'builtin:zai-coding-plan',
                         'GLM-5.2', 'max', 'zcode-agent', 'completed', 1000)",
                [],
            )
            .unwrap();

        let snapshot = read_zcode_snapshot_from_connection(&connection, 100_000, 6).unwrap();
        let task = &snapshot["tasks"][0];
        assert_eq!(task["state"], "completed");
        assert_eq!(task["priority"], 40);
    }

    #[test]
    fn recent_completed_session_is_immediately_completed() {
        let connection = fixture_db();
        connection
            .execute(
                "INSERT INTO session (id, project_id, slug, directory, title, version,
                                      time_created, time_updated, task_type)
                 VALUES ('sess_c', 'p1', 'c', 'D:\\proj\\other', 'Recent task', '1',
                         1000, 10000, 'interactive')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO model_usage (id, logical_request_id, session_id, provider_id,
                                          model_id, variant, agent, status, started_at)
                 VALUES ('mu3', 'r3', 'sess_c', 'builtin:zai-coding-plan',
                         'GLM-5.2', 'max', 'zcode-agent', 'completed', 9000)",
                [],
            )
            .unwrap();

        let snapshot = read_zcode_snapshot_from_connection(&connection, 15_000, 6).unwrap();
        let task = &snapshot["tasks"][0];
        assert_eq!(task["state"], "completed");
    }

    #[test]
    fn completed_session_is_green_phase_before_returning_to_idle() {
        let latest = ModelUsage {
            status: "completed".to_owned(),
            model_id: None,
            variant: None,
            started_at: 1_000,
            completed_at: Some(10_000),
        };

        assert_eq!(
            derive_state(Some(&latest), None, 10_000, 25_000),
            "completed"
        );
        assert_eq!(
            derive_state(Some(&latest), None, 10_000, 45_000),
            "completed"
        );
    }

    #[test]
    fn snapshot_limit_keeps_only_the_most_recent_sessions() {
        let connection = fixture_db();
        connection
            .execute_batch(
                "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated, task_type)
                 VALUES ('old', 'p1', 'old', 'D:\\proj', 'Old', '1', 1, 1, 'interactive'),
                        ('mid', 'p1', 'mid', 'D:\\proj', 'Mid', '1', 2, 2, 'interactive'),
                        ('new', 'p1', 'new', 'D:\\proj', 'New', '1', 3, 3, 'interactive');
                 INSERT INTO model_usage (id, logical_request_id, session_id, provider_id, model_id, status, started_at)
                 VALUES ('mu-old', 'r-old', 'old', 'provider', 'model', 'completed', 1),
                        ('mu-mid', 'r-mid', 'mid', 'provider', 'model', 'completed', 2),
                        ('mu-new', 'r-new', 'new', 'provider', 'model', 'completed', 3);",
            )
            .unwrap();

        let snapshot = read_zcode_snapshot_from_connection(&connection, 100_000, 2).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0]["task_id"], "zcode:new");
        assert_eq!(tasks[1]["task_id"], "zcode:mid");
    }

    #[test]
    fn running_session_wins_a_capacity_limited_snapshot() {
        let connection = fixture_db();
        connection
            .execute_batch(
                "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated, task_type)
                 VALUES ('running-old', 'p1', 'running-old', 'D:\\proj', 'Running', '1', 1, 1, 'interactive'),
                        ('idle-new', 'p1', 'idle-new', 'D:\\proj', 'Idle', '1', 2, 100, 'interactive');
                 INSERT INTO model_usage (id, logical_request_id, session_id, provider_id, model_id, status, started_at, completed_at)
                 VALUES ('mu-running', 'r-running', 'running-old', 'provider', 'model', 'running', 1, NULL),
                        ('mu-idle', 'r-idle', 'idle-new', 'provider', 'model', 'completed', 100, 100);",
            )
            .unwrap();

        let snapshot = read_zcode_snapshot_from_connection(&connection, 100, 1).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "zcode:running-old");
        assert_eq!(tasks[0]["state"], "running");
    }

    #[test]
    fn equal_start_times_use_the_last_inserted_usage_row() {
        let connection = fixture_db();
        connection
            .execute_batch(
                "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated, task_type)
                 VALUES ('tie', 'p1', 'tie', 'D:\\proj', 'Tie', '1', 1, 10, 'interactive');
                 INSERT INTO model_usage (id, logical_request_id, session_id, provider_id, model_id, status, started_at, completed_at)
                 VALUES ('first', 'r1', 'tie', 'provider', 'model', 'completed', 10, 10),
                        ('second', 'r2', 'tie', 'provider', 'model', 'running', 10, NULL);",
            )
            .unwrap();

        let snapshot = read_zcode_snapshot_from_connection(&connection, 10, 1).unwrap();
        assert_eq!(snapshot["tasks"][0]["state"], "running");
    }

    #[test]
    fn unknown_status_is_visible_as_error() {
        let latest = ModelUsage {
            status: "unexpected".to_owned(),
            model_id: None,
            variant: None,
            started_at: 1,
            completed_at: None,
        };

        assert_eq!(derive_state(Some(&latest), None, 1, 1), "error");
    }

    #[test]
    fn streaming_tail_maps_terminal_status_to_running() {
        // The latest model request finished, but the session's newest
        // assistant message has no completion timestamp: the next request is
        // still generating.
        let connection = fixture_db();
        insert_session(&connection, "sess_stream", 10_000);
        insert_usage(
            &connection,
            "mu1",
            "sess_stream",
            "completed",
            9_000,
            Some(9_500),
        );
        insert_message(&connection, "sess_stream", 1, 9_600, "user", None, None);
        insert_message(
            &connection,
            "sess_stream",
            2,
            10_500,
            "assistant",
            None,
            None,
        );

        let snapshot = read_zcode_snapshot_from_connection(&connection, 11_000, 6).unwrap();
        let task = &snapshot["tasks"][0];
        assert_eq!(task["state"], "running");
        assert_eq!(task["priority"], 75);
        assert_eq!(task["started_at_ms"], 9_000);
    }

    #[test]
    fn tool_call_tail_keeps_turn_running() {
        // The newest assistant message completed with finish=tool-calls:
        // tools are executing and the turn is still in flight.
        let connection = fixture_db();
        insert_session(&connection, "sess_tools", 10_000);
        insert_usage(
            &connection,
            "mu1",
            "sess_tools",
            "completed",
            9_000,
            Some(10_400),
        );
        insert_message(&connection, "sess_tools", 1, 9_000, "user", None, None);
        insert_message(
            &connection,
            "sess_tools",
            2,
            10_500,
            "assistant",
            Some("tool-calls"),
            Some(10_480),
        );

        let snapshot = read_zcode_snapshot_from_connection(&connection, 11_000, 6).unwrap();
        assert_eq!(snapshot["tasks"][0]["state"], "running");
    }

    #[test]
    fn stopped_tail_falls_back_to_terminal_status() {
        // The newest assistant message finished with stop: the turn is over,
        // so the terminal model-usage status decides.
        let connection = fixture_db();
        insert_session(&connection, "sess_done", 10_000);
        insert_usage(
            &connection,
            "mu1",
            "sess_done",
            "completed",
            9_000,
            Some(10_400),
        );
        insert_message(&connection, "sess_done", 1, 9_000, "user", None, None);
        insert_message(
            &connection,
            "sess_done",
            2,
            10_500,
            "assistant",
            Some("stop"),
            Some(10_480),
        );

        let snapshot = read_zcode_snapshot_from_connection(&connection, 11_000, 6).unwrap();
        assert_eq!(snapshot["tasks"][0]["state"], "completed");
    }

    #[test]
    fn stale_open_tail_is_not_running() {
        // An aborted turn leaves its streaming message uncompleted forever.
        // Past the recency window the terminal status wins again.
        let connection = fixture_db();
        insert_session(&connection, "sess_stale", 10_000);
        insert_usage(
            &connection,
            "mu1",
            "sess_stale",
            "completed",
            9_000,
            Some(9_500),
        );
        insert_message(
            &connection,
            "sess_stale",
            1,
            10_500,
            "assistant",
            None,
            None,
        );

        let snapshot =
            read_zcode_snapshot_from_connection(&connection, 10_000 + RUNNING_WINDOW_MS + 1, 6)
                .unwrap();
        assert_eq!(snapshot["tasks"][0]["state"], "completed");
    }

    #[test]
    fn session_without_model_usage_publishes_running_turn() {
        // A brand-new session: the prompt is persisted but no model request
        // has finished yet.  The task board still shows it as running.
        let connection = fixture_db();
        insert_session(&connection, "sess_fresh", 10_000);
        insert_message(&connection, "sess_fresh", 1, 9_900, "user", None, None);

        let snapshot = read_zcode_snapshot_from_connection(&connection, 11_000, 6).unwrap();
        let task = &snapshot["tasks"][0];
        assert_eq!(task["task_id"], "zcode:sess_fresh");
        assert_eq!(task["state"], "running");
        assert_eq!(task["started_at_ms"], 10_000);
    }

    #[test]
    fn missing_database_returns_none() {
        // Point at a path that does not exist.
        let snapshot = Connection::open_with_flags(
            "/nonexistent/zcode-db-does-not-exist.sqlite",
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()
        .and_then(|_| read_zcode_snapshot(0, 6));
        assert!(snapshot.is_none());
    }
}
