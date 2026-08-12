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

const ACTIVITY_LIVENESS: Duration = Duration::from_secs(10);

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
    let path = hermes_db_path()?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    connection.busy_timeout(Duration::from_millis(100)).ok()?;
    read_hermes_snapshot_from_connection(&connection, now_ms, max_tasks)
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
}

fn message_activity(connection: &Connection, session_id: &str) -> MessageActivity {
    let latest = connection
        .query_row(
            "SELECT role, timestamp, finish_reason FROM messages \
             WHERE session_id = ?1 ORDER BY timestamp DESC, id DESC LIMIT 1",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, Option<String>>(2)?,
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
    MessageActivity {
        latest_role: latest.as_ref().map(|value| value.0.clone()),
        latest_at: latest.as_ref().map(|value| value.1),
        finish_reason: latest.and_then(|value| value.2),
        latest_user_at,
    }
}

fn build_task(connection: &Connection, row: &SessionRow, now_ms: u128) -> Value {
    let activity = message_activity(connection, &row.id);
    let (state, started_at_ms, finished_at_ms) = derive_state(row, &activity, now_ms);
    let task_id = format!("hermes:{}", row.id);
    let project = row.cwd.as_deref().map(project_name);
    let effort = row.model_config.as_deref().and_then(model_effort);
    let priority = if state == "running" { 75 } else { 40 };
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
                content text, timestamp real not null, finish_reason text
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
             INSERT INTO messages VALUES
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
}
