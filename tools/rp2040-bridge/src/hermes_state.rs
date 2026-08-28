//! Read-only access to Hermes Agent's canonical session database.
//!
//! Hermes stores session metadata and messages in `state.db`. The daemon
//! mirrors recent sessions onto the shared task board while a Hermes MCP
//! proxy is connected. Every query is read-only; a missing, locked, or older
//! database simply disables the auto-feed without disturbing existing cards.

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::tasks::{TASK_PRIORITY_ACTIVE, TASK_PRIORITY_IDLE};

/// How recently a session must have shown message activity for a structurally
/// open turn to still count as running. Tool executions between model calls
/// can pause message writes for minutes, so this matches ZCode's
/// `RUNNING_WINDOW_MS` (see zcode_state.rs).
const ACTIVITY_LIVENESS: Duration = Duration::from_secs(300);

pub fn hermes_db_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HERMES_HOME") {
        return Some(Path::new(&home).join("state.db"));
    }
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return Some(Path::new(&local).join("hermes").join("state.db"));
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(Path::new(&home).join(".hermes").join("state.db"))
}

pub fn read_hermes_snapshot(now_ms: u128, max_tasks: usize) -> Option<Value> {
    let local = hermes_db_path().and_then(|path| {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        connection.busy_timeout(Duration::from_millis(100)).ok()?;
        read_hermes_snapshot_from_connection(&connection, now_ms, max_tasks)
    });
    if local
        .as_ref()
        .and_then(|snapshot| snapshot.get("tasks"))
        .and_then(Value::as_array)
        .is_some_and(|tasks| !tasks.is_empty())
    {
        return local;
    }
    read_remote_cache_snapshot(max_tasks).or(local)
}

/// Hermes Desktop can point at an OAuth-protected remote backend. In that
/// mode the local state.db is intentionally empty, but Electron caches the
/// already-authenticated sidebar response as a plain JSON response body. Read
/// that response as a credential-free fallback so remote sessions remain
/// visible without duplicating Chromium's OAuth cookie handling in the bridge.
fn read_remote_cache_snapshot(max_tasks: usize) -> Option<Value> {
    if max_tasks == 0 {
        return Some(json!({"tasks": []}));
    }
    let roaming = std::env::var("APPDATA").ok()?;
    let cache_dir = Path::new(&roaming)
        .join("Hermes")
        .join("Partitions")
        .join("hermes-remote-oauth")
        .join("Cache")
        .join("Cache_Data");
    read_remote_cache_snapshot_from(&cache_dir, max_tasks)
}

fn read_remote_cache_snapshot_from(cache_dir: &Path, max_tasks: usize) -> Option<Value> {
    let mut candidates = std::fs::read_dir(cache_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("f_") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 {
                return None;
            }
            Some((metadata.modified().ok()?, entry.path()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(modified, _)| *modified);

    for (_, path) in candidates.into_iter().rev().take(64) {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(response) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        // The backend moved the sidebar listing from `recents.sessions` to a
        // top-level `sessions` array. Accept both shapes: otherwise the last
        // cached response in the retired shape (whose sessions are all
        // inactive) masks every fresher listing and the board never sees a
        // running session again.
        let sessions = response
            .get("recents")
            .and_then(|recents| recents.get("sessions"))
            .or_else(|| response.get("sessions"))
            .and_then(Value::as_array);
        let Some(sessions) = sessions else {
            continue;
        };
        let mut sessions = sessions.iter().collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            cached_session_active(right)
                .cmp(&cached_session_active(left))
                .then_with(|| {
                    cached_session_activity(right)
                        .partial_cmp(&cached_session_activity(left))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        let tasks = sessions
            .into_iter()
            .filter_map(|session| build_cached_task(session))
            .take(max_tasks)
            .collect::<Vec<_>>();
        return Some(json!({"tasks": tasks}));
    }
    None
}

fn cached_session_active(session: &Value) -> bool {
    session
        .get("is_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn cached_session_activity(session: &Value) -> f64 {
    session
        .get("last_active")
        .or_else(|| session.get("last_activity_at"))
        .or_else(|| session.get("started_at"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// Detects only explicit approval markers supplied by a Hermes backend or
/// persisted message metadata. A generic active session is deliberately not
/// treated as waiting: tool execution and approval pauses look identical in
/// the lifecycle fields Hermes persists to its local database.
fn value_has_pending_approval_marker(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    for key in [
        "pending_approval",
        "approval_pending",
        "approval_required",
        "permission_request",
    ] {
        let Some(marker) = object.get(key) else {
            continue;
        };
        if marker.as_bool() == Some(true) || marker.is_object() || marker.is_array() {
            return true;
        }
        if marker.as_str().is_some_and(is_pending_approval_status) {
            return true;
        }
    }
    ["status", "state", "kind", "type", "display_kind"]
        .into_iter()
        .filter_map(|key| object.get(key).and_then(Value::as_str))
        .any(is_pending_approval_status)
}

fn is_pending_approval_status(value: &str) -> bool {
    matches!(
        value
            .trim()
            .to_ascii_lowercase()
            .replace('-', "_")
            .replace(' ', "_")
            .as_str(),
        "approval"
            | "approval_pending"
            | "approval_required"
            | "approval_request"
            | "awaiting_approval"
            | "needs_approval"
            | "permission_request"
            | "permission_required"
            | "pending_approval"
    )
}

fn build_cached_task(session: &Value) -> Option<Value> {
    let id = session.get("id")?.as_str()?;
    let source = session
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(source, "delegate" | "batch")
        || session
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return None;
    }
    let title = ["title", "display_name", "preview"]
        .into_iter()
        .find_map(|key| {
            session
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or(id);
    let task_id = format!("hermes:{id}");
    let cwd = session.get("cwd").and_then(Value::as_str);
    let project = cwd.map(project_name).or_else(|| {
        session
            .get("profile")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let model = session.get("model").and_then(Value::as_str);
    let approval_pending = value_has_pending_approval_marker(session);
    let active = cached_session_active(session);
    let end_reason = session
        .get("end_reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let failed = end_reason.contains("error") || end_reason.contains("fail");
    // The remote backend flips is_active off the moment a turn finishes, but
    // it records ended_at only on abnormal closes (ws_orphan_reap,
    // cli_close). Requiring ended_at for completion left every naturally
    // finished session stuck in queued, so its card never turned green.
    let state = if approval_pending {
        "waiting"
    } else if active {
        "running"
    } else if failed {
        "error"
    } else {
        "completed"
    };
    let priority = if active || approval_pending {
        TASK_PRIORITY_ACTIVE
    } else {
        TASK_PRIORITY_IDLE
    };
    // The cached listing only dates the session, never the current turn, so
    // any explicit timestamp would render the session's age as the running
    // timer. Publishing without timestamps makes the task board snapshot its
    // own first-observed running time and keep it until the turn completes.
    Some(json!({
        "task_id": task_id,
        "title": title,
        "project": project,
        "workspace_path": cwd,
        "model": model,
        "state": state,
        "priority": priority,
        "context": {
            "project": project,
            "task": title,
            "model": model,
            "status": state,
            "task_id": task_id,
        }
    }))
}

fn read_hermes_snapshot_from_connection(
    connection: &Connection,
    now_ms: u128,
    max_tasks: usize,
) -> Option<Value> {
    if max_tasks == 0 {
        return Some(json!({"tasks": []}));
    }
    let columns = table_columns(connection, "sessions")?;
    for required in [
        "id",
        "source",
        "model",
        "started_at",
        "ended_at",
        "end_reason",
        "title",
    ] {
        if !columns.iter().any(|column| column == required) {
            return None;
        }
    }
    let cwd = if columns.iter().any(|column| column == "cwd") {
        "NULLIF(cwd, '')"
    } else {
        "NULL"
    };
    let model_config = if columns.iter().any(|column| column == "model_config") {
        "model_config"
    } else {
        "NULL"
    };
    let last_activity = if columns.iter().any(|column| column == "last_activity_at") {
        "COALESCE(last_activity_at, ended_at, started_at)"
    } else {
        "COALESCE(ended_at, started_at)"
    };
    let archived = if columns.iter().any(|column| column == "archived") {
        "COALESCE(archived, 0) = 0"
    } else {
        "1 = 1"
    };
    let sql = format!(
        "SELECT id, COALESCE(NULLIF(title, ''), NULLIF(display_name, ''), id), \
         {cwd}, NULLIF(model, ''), {model_config}, started_at, ended_at, end_reason, \
         {last_activity} FROM sessions WHERE {archived} \
         AND source NOT IN ('delegate', 'batch') ORDER BY {last_activity} DESC LIMIT ?1"
    );
    let mut statement = connection.prepare(&sql).ok()?;
    let rows = statement
        .query_map([i64::try_from(max_tasks).unwrap_or(i64::MAX)], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                title: row.get(1)?,
                cwd: row.get(2)?,
                model: row.get(3)?,
                model_config: row.get(4)?,
                started_at: row.get(5)?,
                ended_at: row.get(6)?,
                end_reason: row.get(7)?,
                last_activity_at: row.get(8)?,
            })
        })
        .ok()?;

    let tasks = rows
        .flatten()
        .map(|row| build_task(connection, &row, now_ms))
        .collect::<Vec<_>>();
    Some(json!({"tasks": tasks}))
}

fn table_columns(connection: &Connection, table: &str) -> Option<Vec<String>> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .ok()?;
    let rows = statement.query_map([], |row| row.get(1)).ok()?;
    Some(rows.flatten().collect())
}

struct SessionRow {
    id: String,
    title: String,
    cwd: Option<String>,
    model: Option<String>,
    model_config: Option<String>,
    started_at: f64,
    ended_at: Option<f64>,
    end_reason: Option<String>,
    last_activity_at: f64,
}

#[derive(Default)]
struct MessageActivity {
    latest_role: Option<String>,
    latest_at: Option<f64>,
    finish_reason: Option<String>,
    latest_user_at: Option<f64>,
    approval_pending: bool,
}

fn message_activity(connection: &Connection, session_id: &str) -> MessageActivity {
    let columns = table_columns(connection, "messages").unwrap_or_default();
    let display_kind = if columns.iter().any(|column| column == "display_kind") {
        "display_kind"
    } else {
        "NULL"
    };
    let display_metadata = if columns.iter().any(|column| column == "display_metadata") {
        "display_metadata"
    } else {
        "NULL"
    };
    let latest = connection
        .query_row(
            &format!(
                "SELECT role, timestamp, finish_reason, {display_kind}, {display_metadata} \
                 FROM messages WHERE session_id = ?1 \
                 ORDER BY timestamp DESC, id DESC LIMIT 1"
            ),
            [session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .ok();
    let latest_user_at = connection
        .query_row(
            "SELECT MAX(timestamp) FROM messages WHERE session_id = ?1 AND role = 'user'",
            [session_id],
            |row| row.get::<_, Option<f64>>(0),
        )
        .ok()
        .flatten();
    let approval_pending = latest.as_ref().is_some_and(|value| {
        let kind_pending = value.3.as_deref().is_some_and(is_pending_approval_status);
        let metadata_pending = value
            .4
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .is_some_and(|metadata| value_has_pending_approval_marker(&metadata));
        kind_pending || metadata_pending
    });
    MessageActivity {
        latest_role: latest.as_ref().map(|value| value.0.clone()),
        latest_at: latest.as_ref().map(|value| value.1),
        finish_reason: latest.and_then(|value| value.2),
        latest_user_at,
        approval_pending,
    }
}

fn build_task(connection: &Connection, row: &SessionRow, now_ms: u128) -> Value {
    let activity = message_activity(connection, &row.id);
    let (state, started_at_ms, finished_at_ms) = derive_state(row, &activity, now_ms);
    let task_id = format!("hermes:{}", row.id);
    let project = row.cwd.as_deref().map(project_name);
    let effort = row.model_config.as_deref().and_then(model_effort);
    let priority = if matches!(state, "running" | "waiting") {
        TASK_PRIORITY_ACTIVE
    } else {
        TASK_PRIORITY_IDLE
    };
    json!({
        "task_id": task_id,
        "title": row.title,
        "project": project,
        "workspace_path": row.cwd,
        "model": row.model,
        "effort": effort,
        "state": state,
        "priority": priority,
        "started_at_ms": started_at_ms,
        "finished_at_ms": finished_at_ms,
        "timing_authoritative": true,
        "context": {
            "project": project,
            "task": row.title,
            "model": row.model,
            "effort": effort,
            "status": state,
            "task_id": task_id,
        }
    })
}

fn derive_state(
    row: &SessionRow,
    activity: &MessageActivity,
    now_ms: u128,
) -> (&'static str, Option<u128>, Option<u128>) {
    let latest_ms = seconds_to_ms(activity.latest_at.unwrap_or(row.last_activity_at));
    let started_ms = seconds_to_ms(activity.latest_user_at.unwrap_or(row.started_at));
    let elapsed = now_ms.saturating_sub(latest_ms);
    if activity.approval_pending && elapsed <= ACTIVITY_LIVENESS.as_millis() {
        return ("waiting", Some(started_ms), None);
    }
    let finish = activity.finish_reason.as_deref().unwrap_or_default();
    let pending = matches!(activity.latest_role.as_deref(), Some("user" | "tool"))
        || (activity.latest_role.as_deref() == Some("assistant")
            && !matches!(finish, "stop" | "completed" | "length"));
    if pending && elapsed <= ACTIVITY_LIVENESS.as_millis() {
        return ("running", Some(started_ms), None);
    }
    let failed = row
        .end_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("error") || reason.contains("fail"));
    if failed {
        return ("error", Some(started_ms), row.ended_at.map(seconds_to_ms));
    }
    let completed = activity.latest_role.as_deref() == Some("assistant")
        && matches!(finish, "stop" | "completed" | "length");
    if completed {
        return ("completed", Some(started_ms), Some(latest_ms));
    }
    ("queued", None, None)
}

fn seconds_to_ms(value: f64) -> u128 {
    if value <= 0.0 {
        0
    } else {
        (value * 1000.0).round() as u128
    }
}

fn project_name(directory: &str) -> String {
    Path::new(directory)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(directory)
        .to_owned()
}

fn model_effort(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    ["reasoning_effort", "effort", "thinking_level", "reasoning"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str).map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::SystemTime;

    fn fixture_db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                id text primary key, source text not null, display_name text,
                model text, model_config text, started_at real not null,
                ended_at real, end_reason text, title text, cwd text,
                last_activity_at real, archived integer default 0
             );
             CREATE TABLE messages (
                id integer primary key, session_id text not null, role text not null,
                content text, timestamp real not null, finish_reason text,
                display_kind text, display_metadata text
             );",
            )
            .unwrap();
        connection
    }

    #[test]
    fn maps_running_and_completed_sessions_without_writing() {
        let connection = fixture_db();
        connection.execute_batch(
            "INSERT INTO sessions VALUES
             ('run','desktop',NULL,'hermes-4','{\"reasoning_effort\":\"high\"}',1,NULL,NULL,'Build parity','D:\\repo\\micro-emu',5,0),
             ('done','cli',NULL,'hermes-4',NULL,1,NULL,NULL,'Finished','D:\\repo\\other',4,0);
             INSERT INTO messages
             (id,session_id,role,content,timestamp,finish_reason) VALUES
             (1,'run','user','go',5,NULL),
             (2,'done','user','go',2,NULL),
             (3,'done','assistant','ok',4,'stop');"
        ).unwrap();
        let snapshot = read_hermes_snapshot_from_connection(&connection, 6_000, 6).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks[0]["task_id"], "hermes:run");
        assert_eq!(tasks[0]["state"], "running");
        assert_eq!(tasks[0]["effort"], "high");
        assert_eq!(tasks[0]["project"], "micro-emu");
        assert_eq!(tasks[1]["state"], "completed");
        assert_eq!(tasks[1]["finished_at_ms"], 4_000);
    }

    #[test]
    fn archived_and_delegate_sessions_are_excluded_and_limit_is_honored() {
        let connection = fixture_db();
        connection
            .execute_batch(
                "INSERT INTO sessions VALUES
             ('old','cli',NULL,NULL,NULL,1,NULL,NULL,'Old',NULL,1,0),
             ('new','desktop',NULL,NULL,NULL,2,NULL,NULL,'New',NULL,3,0),
             ('delegate','delegate',NULL,NULL,NULL,3,NULL,NULL,'Hidden',NULL,4,0),
             ('archived','cli',NULL,NULL,NULL,4,NULL,NULL,'Archived',NULL,5,1);",
            )
            .unwrap();
        let snapshot = read_hermes_snapshot_from_connection(&connection, 10_000, 1).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "hermes:new");
    }

    #[test]
    fn cached_remote_active_session_maps_to_running_task() {
        let task = build_cached_task(&json!({
            "id": "remote-1",
            "source": "desktop",
            "title": "Remote work",
            "model": "hermes-4",
            "profile": "default",
            "started_at": 10.0,
            "last_active": 12.0,
            "ended_at": null,
            "archived": false,
            "is_active": true
        }))
        .unwrap();
        assert_eq!(task["task_id"], "hermes:remote-1");
        assert_eq!(task["state"], "running");
        assert_eq!(task["project"], "default");
        // No explicit timing: the board times the turn from its own
        // first observation instead of rendering the session's age.
        assert!(task.get("started_at_ms").is_none());
        assert!(task.get("finished_at_ms").is_none());
    }

    #[test]
    fn explicit_remote_approval_marker_maps_to_waiting_task() {
        let task = build_cached_task(&json!({
            "id": "remote-approval",
            "title": "Needs approval",
            "started_at": 10.0,
            "last_active": 12.0,
            "is_active": true,
            "pending_approval": {"tool": "shell"}
        }))
        .unwrap();
        assert_eq!(task["state"], "waiting");
        assert_eq!(task["priority"], 75);
    }

    #[test]
    fn explicit_message_approval_marker_maps_to_waiting_task() {
        let connection = fixture_db();
        connection
            .execute(
                "INSERT INTO sessions VALUES
                 ('approval','desktop',NULL,'hermes-4',NULL,1,NULL,NULL,
                  'Needs approval','D:\\repo\\micro-emu',5,0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages
                 (id, session_id, role, content, timestamp, finish_reason, display_kind)
                 VALUES (1, 'approval', 'assistant', '', 5, 'stop', 'approval_required')",
                [],
            )
            .unwrap();

        let snapshot = read_hermes_snapshot_from_connection(&connection, 6_000, 6).unwrap();
        let task = &snapshot["tasks"][0];
        assert_eq!(task["state"], "waiting");
        assert_eq!(task["priority"], 75);
    }

    #[test]
    fn cached_remote_finished_session_maps_to_completed_task() {
        // A naturally finished turn: the backend clears is_active but only
        // writes ended_at on abnormal closes, so ended_at stays empty.
        let task = build_cached_task(&json!({
            "id": "remote-2",
            "source": "desktop",
            "title": "Finished work",
            "started_at": 10.0,
            "last_active": 15.0,
            "ended_at": null,
            "end_reason": null,
            "is_active": false
        }))
        .unwrap();
        assert_eq!(task["state"], "completed");
        assert!(task.get("finished_at_ms").is_none());
    }

    #[test]
    fn cached_remote_reaped_session_still_maps_to_completed_task() {
        let task = build_cached_task(&json!({
            "id": "remote-3",
            "title": "Reaped work",
            "started_at": 10.0,
            "last_active": 12.0,
            "ended_at": 14.0,
            "end_reason": "ws_orphan_reap",
            "is_active": false
        }))
        .unwrap();
        assert_eq!(task["state"], "completed");
        assert!(task.get("finished_at_ms").is_none());
    }

    #[test]
    fn cached_remote_failed_session_maps_to_error_task() {
        let task = build_cached_task(&json!({
            "id": "remote-4",
            "title": "Failed work",
            "started_at": 10.0,
            "last_active": 12.0,
            "ended_at": 14.0,
            "end_reason": "agent_error",
            "is_active": false
        }))
        .unwrap();
        assert_eq!(task["state"], "error");
    }

    /// End-to-end check of the timing contract: a remote card picked up
    /// mid-turn starts its timer when first observed, keeps it across
    /// republished listings, and finishes with the observed turn length
    /// instead of the session's age.
    #[test]
    fn board_times_remote_cards_from_first_observation() {
        let mut board = crate::tasks::TaskBoard::new();
        board.set_device("deck", 1, true);
        let publish = |board: &mut crate::tasks::TaskBoard, session: Value, now_ms: u128| {
            board
                .publish_tasks(
                    990,
                    crate::routing::AgentId::Hermes,
                    &json!({"tasks": [session]}),
                    now_ms,
                )
                .unwrap();
        };
        let running = || {
            build_cached_task(&json!({
                "id": "timed",
                "title": "Timed work",
                "started_at": 10.0,
                "is_active": true
            }))
            .unwrap()
        };
        publish(&mut board, running(), 1_000);
        assert_eq!(
            board.task("hermes:timed").unwrap().started_at_ms,
            Some(1_000)
        );

        // A later listing of the still-running session must not restart the timer.
        publish(&mut board, running(), 5_000);
        assert_eq!(
            board.task("hermes:timed").unwrap().started_at_ms,
            Some(1_000)
        );

        let finished = build_cached_task(&json!({
            "id": "timed",
            "title": "Timed work",
            "started_at": 10.0,
            "is_active": false
        }))
        .unwrap();
        publish(&mut board, finished, 9_000);
        let card = board.task("hermes:timed").unwrap();
        assert_eq!(card.state, crate::tasks::TaskState::Completed);
        assert_eq!(card.started_at_ms, Some(1_000));
        assert_eq!(card.finished_at_ms, Some(9_000));
    }

    fn temp_cache_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hermes-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_cache_entry(dir: &Path, name: &str, modified: SystemTime, body: Value) {
        let path = dir.join(name);
        std::fs::write(&path, body.to_string()).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[test]
    fn top_level_sessions_listing_feeds_the_task_board() {
        let dir = temp_cache_dir("top-level");
        write_cache_entry(
            &dir,
            "f_listing",
            SystemTime::now(),
            json!({
                "sessions": [{
                    "id": "live-1",
                    "source": "desktop",
                    "title": "Live work",
                    "started_at": 10.0,
                    "last_active": 12.0,
                    "is_active": true
                }],
                "total": 1
            }),
        );
        let snapshot = read_remote_cache_snapshot_from(&dir, 6).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "hermes:live-1");
        assert_eq!(tasks[0]["state"], "running");
    }

    #[test]
    fn fresh_top_level_listing_wins_over_stale_recents_entry() {
        let dir = temp_cache_dir("mixed-shapes");
        let now = SystemTime::now();
        write_cache_entry(
            &dir,
            "f_stale",
            now - Duration::from_secs(60),
            json!({"recents": {"sessions": [{
                "id": "stale-1",
                "title": "Stale work",
                "is_active": false
            }]}}),
        );
        write_cache_entry(
            &dir,
            "f_fresh",
            now,
            json!({"sessions": [{
                "id": "fresh-1",
                "title": "Fresh work",
                "is_active": true
            }]}),
        );
        let snapshot = read_remote_cache_snapshot_from(&dir, 6).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "hermes:fresh-1");
        assert_eq!(tasks[0]["state"], "running");
    }

    #[test]
    fn legacy_recents_listing_still_feeds_the_task_board() {
        let dir = temp_cache_dir("legacy");
        write_cache_entry(
            &dir,
            "f_listing",
            SystemTime::now(),
            json!({"recents": {"sessions": [{
                "id": "legacy-1",
                "title": "Legacy work",
                "last_active": 5.0,
                "is_active": false
            }]}}),
        );
        let snapshot = read_remote_cache_snapshot_from(&dir, 6).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks[0]["task_id"], "hermes:legacy-1");
        assert_eq!(tasks[0]["state"], "completed");
    }
}
