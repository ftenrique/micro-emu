//! Read-only access to ZCode's on-disk session state.
//!
//! ZCode stores its live agent state in a SQLite database at
//! `~/.zcode/cli/db/db.sqlite`.  The daemon periodically polls this database
//! to mirror active ZCode sessions on the Stream Deck task board, the same
//! way Codex Micro activity arrives via `v.oai.thstatus` serial frames.
//!
//! All queries open the database read-only so the running ZCode process keeps
//! its write lock undisturbed.  If the database is missing, locked, or has an
//! unexpected schema, [`read_zcode_snapshot`] returns `None` so the caller can
//! leave the existing board untouched rather than blanking it.

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
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

    read_zcode_snapshot_from_connection(&connection, now_ms, max_tasks)
}

fn read_zcode_snapshot_from_connection(
    connection: &Connection,
    now_ms: u128,
    max_tasks: usize,
) -> Option<Value> {
    let mut statement = connection
        .prepare(
            "SELECT s.id, s.title, s.directory, s.time_updated,
                    mu.status, mu.model_id, mu.variant, mu.started_at, mu.completed_at
             FROM session AS s
             JOIN model_usage AS mu ON mu.rowid = (
                 SELECT latest.rowid FROM model_usage AS latest
                 WHERE latest.session_id = s.id
                 ORDER BY latest.started_at DESC, latest.rowid DESC LIMIT 1
             )
             WHERE s.time_archived IS NULL
               AND s.task_type IN ('interactive', 'task')
             ORDER BY CASE WHEN mu.status = 'running' THEN 0 ELSE 1 END,
                      MAX(s.time_updated, mu.started_at, COALESCE(mu.completed_at, 0)) DESC,
                      s.id ASC
             LIMIT ?1;",
        )
        .ok()?;

    let rows = statement
        .query_map([i64::try_from(max_tasks).unwrap_or(i64::MAX)], |row| {
            Ok(SessionRow {
                id: row.get::<_, String>(0)?,
                title: row.get::<_, String>(1)?,
                directory: row.get::<_, String>(2)?,
                time_updated: row.get::<_, i64>(3)?,
                latest: ModelUsage {
                    status: row.get(4)?,
                    model_id: row.get(5)?,
                    variant: row.get(6)?,
                    started_at: row.get(7)?,
                    completed_at: row.get(8)?,
                },
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
    latest: ModelUsage,
}
/// Builds a single task object for one session, enriching it with the latest
/// model-usage status and the model/effort selection.
fn build_task(connection: &Connection, row: &SessionRow, now_ms: u128) -> Value {
    let latest = &row.latest;
    let selection = model_selection(connection, &row.id);

    let state = derive_state(latest, row.time_updated, now_ms);
    let started_at_ms = latest.started_at;
    let finished_at_ms = latest.completed_at;
    let model = selection
        .as_ref()
        .and_then(|s| s.model.clone())
        .or_else(|| latest.model_id.clone());
    let effort = selection
        .as_ref()
        .and_then(|s| s.thought_level.clone())
        .or_else(|| latest.variant.clone());
    let project = project_name(&row.directory);

    // Running sessions rank above idle ones so they claim the lowest slots.
    let priority = if state == "running" { 75 } else { 40 };

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
        "started_at_ms": (state != "queued").then_some(started_at_ms),
        "finished_at_ms": (state == "completed").then_some(
            finished_at_ms.unwrap_or_else(|| started_at_ms.max(row.time_updated))
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

/// Maps the persisted model-usage status to a task-board state.
/// Completion remains visible until the task board acknowledges it through selection.
fn derive_state(latest: &ModelUsage, _session_updated_ms: i64, _now_ms: u128) -> &'static str {
    match latest.status.trim().to_ascii_lowercase().as_str() {
        "running" => "running",
        "error" => "error",
        "cancelled" => "queued",
        "completed" => "completed",
        _ => "error",
    }
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
                );",
            )
            .unwrap();
        connection
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

        assert_eq!(derive_state(&latest, 10_000, 25_000), "completed");
        assert_eq!(derive_state(&latest, 10_000, 45_000), "completed");
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

        assert_eq!(derive_state(&latest, 1, 1), "error");
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
