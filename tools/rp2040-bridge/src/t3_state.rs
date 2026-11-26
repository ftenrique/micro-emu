//! Read-only access to T3 Code's local SQLite projection database.
//!
//! T3 Code Desktop persists its orchestration state in
//! `~/.t3/userdata/statev2.sqlite` (or `$T3CODE_HOME/userdata/statev2.sqlite`).
//! The daemon polls this database read-only to mirror active and recent
//! top-level threads onto the shared task board, tagging each card with its
//! underlying provider harness (`T3-Codex`, `T3-Claude`, `T3-AGY`,
//! etc.). The bridge never writes to T3 Code's database or config files.

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::tasks::{TASK_PRIORITY_ACTIVE, TASK_PRIORITY_IDLE, TASK_PRIORITY_SETTLED};

/// Root of T3 Code's per-user data directory (`T3CODE_HOME` or `~/.t3`).
pub fn t3_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("T3CODE_HOME").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|home| !home.is_empty())?;
    Some(Path::new(&home).join(".t3"))
}

/// Path to T3 Code's WAL-backed SQLite state database.
pub fn t3_db_path() -> Option<PathBuf> {
    Some(t3_home()?.join("userdata").join("statev2.sqlite"))
}

/// Reads and strictly validates the local T3 Code environment UUID from
/// `<T3_HOME>/userdata/environment-id`.
pub fn t3_environment_id() -> Option<String> {
    let path = t3_home()?.join("userdata").join("environment-id");
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    is_valid_t3_identifier(trimmed).then(|| trimmed.to_owned())
}

/// Strict identifier check shared by environment IDs and thread IDs before
/// interpolating into `t3code://` deep links or SQLite message queries.
pub fn is_valid_t3_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Formats a T3 provider identifier into the concise harness label rendered on
/// task cards and touch strips (`T3-Codex`, `T3-Claude`, `T3-AGY`,
/// `T3-Devin`, `T3-GLM`, etc.).
pub fn format_provider_harness(provider: &str) -> String {
    let trimmed = provider.trim();
    if trimmed.is_empty() {
        return "T3".to_owned();
    }
    let lower = trimmed.to_ascii_lowercase();
    let suffix = if lower == "codex" || lower.ends_with("_codex") {
        "Codex".to_owned()
    } else if lower == "claudeagent" || lower == "claude" || lower.contains("claude") {
        "Claude".to_owned()
    } else if lower.contains("antigravity") {
        "AGY".to_owned()
    } else if lower.contains("devin") {
        "Devin".to_owned()
    } else if lower.contains("glm") || lower.contains("zcode") || lower.contains("zai") {
        "GLM".to_owned()
    } else if lower.contains("cursor") {
        "Cursor".to_owned()
    } else if lower.contains("opencode") {
        "OpenCode".to_owned()
    } else if lower.contains("gemini") {
        "Gemini".to_owned()
    } else if lower.contains("grok") {
        "Grok".to_owned()
    } else if lower.contains("kiro") {
        "Kiro".to_owned()
    } else if lower == "pi" || lower.ends_with("_pi") || lower.ends_with("-pi") {
        "Pi".to_owned()
    } else {
        let mut cleaned = trimmed;
        for prefix in [
            "acpRegistry_",
            "acpregistry_",
            "acp_",
            "google_",
            "anthropic_",
            "openai_",
        ] {
            if let Some(rest) = cleaned.strip_prefix(prefix) {
                cleaned = rest;
            }
        }
        for strip_suffix in ["_agent", "Agent", "_cli", "Cli"] {
            if let Some(rest) = cleaned.strip_suffix(strip_suffix) {
                cleaned = rest;
            }
        }
        let words: Vec<String> = cleaned
            .split(['_', '-', ' '])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => {
                        let mut out = first.to_uppercase().collect::<String>();
                        out.push_str(chars.as_str());
                        out
                    }
                    None => String::new(),
                }
            })
            .collect();
        if words.is_empty() {
            return "T3".to_owned();
        }
        words.join("")
    };
    format!("T3-{suffix}")
}

type FileStamp = (Option<SystemTime>, u64);
type SnapshotCacheKey = (PathBuf, usize, FileStamp, FileStamp);

fn file_stamp(path: &Path) -> FileStamp {
    match std::fs::metadata(path) {
        Ok(meta) => (meta.modified().ok(), meta.len()),
        Err(_) => (None, 0),
    }
}

/// Reads T3 Code's live threads as a `publish_tasks` snapshot holding at most
/// `max_tasks` cards. Uses an mtime+length cache on both `statev2.sqlite` and
/// `statev2.sqlite-wal` so idle polling performs zero SQLite work.
pub fn read_t3_snapshot(max_tasks: usize) -> Option<Value> {
    let db_path = t3_db_path()?;
    if !db_path.is_file() {
        return None;
    }
    if max_tasks == 0 {
        return Some(json!({"tasks": []}));
    }
    let mut wal_os = db_path.as_os_str().to_os_string();
    wal_os.push("-wal");
    let wal_path = PathBuf::from(wal_os);

    static CACHE: Mutex<Option<(SnapshotCacheKey, Option<Value>)>> = Mutex::new(None);
    let key: SnapshotCacheKey = (
        db_path.clone(),
        max_tasks,
        file_stamp(&db_path),
        file_stamp(&wal_path),
    );
    if let Ok(guard) = CACHE.lock() {
        if let Some((last_key, cached)) = guard.as_ref() {
            if *last_key == key {
                return cached.clone();
            }
        }
    }

    let snapshot = read_t3_snapshot_from_path(&db_path, max_tasks);
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((key, snapshot.clone()));
    }
    snapshot
}

fn read_t3_snapshot_from_path(db_path: &Path, max_tasks: usize) -> Option<Value> {
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    connection.busy_timeout(Duration::from_millis(100)).ok()?;
    read_t3_snapshot_from_connection(&connection, max_tasks)
}

struct ThreadRow {
    thread_id: String,
    title: String,
    default_provider: Option<String>,
    updated_at: String,
    thread_payload: Option<String>,
    project_title: Option<String>,
    workspace_root: Option<String>,
    run_provider: Option<String>,
    run_status: Option<String>,
    run_requested_at: Option<String>,
    run_completed_at: Option<String>,
    run_payload: Option<String>,
    pending_request_id: Option<String>,
    pending_request_kind: Option<String>,
    pending_request_created_at: Option<String>,
    pending_request_payload: Option<String>,
}

fn read_t3_snapshot_from_connection(connection: &Connection, max_tasks: usize) -> Option<Value> {
    if max_tasks == 0 {
        return Some(json!({"tasks": []}));
    }
    // A queued follow-up has a higher ordinal than the run it is waiting for.
    // Use the active run's state, harness, and timing until it finishes.
    let mut statement = connection
        .prepare(
            "SELECT t.thread_id,
                    t.title,
                    t.default_provider,
                    t.updated_at,
                    t.payload_json,
                    p.title,
                    p.workspace_root,
                    r.provider,
                    r.status,
                    r.requested_at,
                    r.completed_at,
                    r.payload_json,
                    rr.runtime_request_id,
                    rr.kind,
                    rr.created_at,
                    rr.payload_json
             FROM orchestration_v2_projection_threads AS t
             LEFT JOIN projection_projects AS p ON p.project_id = t.project_id
             LEFT JOIN orchestration_v2_projection_runs AS r ON r.run_id = COALESCE(
                 (
                     SELECT active.run_id
                     FROM orchestration_v2_projection_runs AS active
                     WHERE active.thread_id = t.thread_id
                       AND active.status IN ('running', 'starting', 'in_progress')
                       AND active.completed_at IS NULL
                     ORDER BY active.ordinal DESC, active.requested_at DESC
                     LIMIT 1
                 ),
                 (
                     SELECT latest.run_id
                     FROM orchestration_v2_projection_runs AS latest
                     WHERE latest.thread_id = t.thread_id
                     ORDER BY latest.ordinal DESC, latest.requested_at DESC
                     LIMIT 1
                 )
             )
             LEFT JOIN orchestration_v2_projection_runtime_requests AS rr ON rr.runtime_request_id = (
                 SELECT pending.runtime_request_id
                 FROM orchestration_v2_projection_runtime_requests AS pending
                 WHERE pending.thread_id = t.thread_id
                   AND pending.resolved_at IS NULL
                 ORDER BY pending.created_at DESC
                 LIMIT 1
             )
             WHERE t.deleted_at IS NULL
               AND t.archived_at IS NULL
               AND t.thread_id NOT LIKE 'import:%'
               AND (
                 NOT json_valid(t.payload_json)
                 OR json_extract(t.payload_json, '$.lineage.parentThreadId') IS NULL
               )
             ORDER BY
               CASE
                 WHEN rr.runtime_request_id IS NOT NULL THEN 0
                 WHEN r.status IN ('running', 'starting', 'in_progress') THEN 1
                 WHEN r.status IN ('failed', 'error') THEN 2
                 WHEN NOT json_valid(t.payload_json)
                      OR (
                        json_extract(t.payload_json, '$.settledAt') IS NULL
                        AND COALESCE(json_extract(t.payload_json, '$.settledOverride'), '') != 'settled'
                      ) THEN 3
                 ELSE 4
               END ASC,
               COALESCE(rr.created_at, r.completed_at, r.requested_at, t.updated_at) DESC,
               t.thread_id ASC
             LIMIT ?1;",
        )
        .ok()?;

    let rows = statement
        .query_map([i64::try_from(max_tasks).unwrap_or(i64::MAX)], |row| {
            Ok(ThreadRow {
                thread_id: row.get(0)?,
                title: row.get(1)?,
                default_provider: row.get(2)?,
                updated_at: row.get(3)?,
                thread_payload: row.get(4)?,
                project_title: row.get(5)?,
                workspace_root: row.get(6)?,
                run_provider: row.get(7)?,
                run_status: row.get(8)?,
                run_requested_at: row.get(9)?,
                run_completed_at: row.get(10)?,
                run_payload: row.get(11)?,
                pending_request_id: row.get(12)?,
                pending_request_kind: row.get(13)?,
                pending_request_created_at: row.get(14)?,
                pending_request_payload: row.get(15)?,
            })
        })
        .ok()?;

    let mut tasks = Vec::new();
    for row in rows.flatten() {
        tasks.push(build_task(&row));
    }
    Some(json!({"tasks": tasks}))
}

fn build_task(row: &ThreadRow) -> Value {
    let thread_payload = row
        .thread_payload
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or(Value::Null);
    let run_payload = row
        .run_payload
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or(Value::Null);

    let settled = thread_payload
        .get("settledAt")
        .is_some_and(|value| !value.is_null())
        || thread_payload
            .get("settledOverride")
            .and_then(Value::as_str)
            == Some("settled");

    let provider = row
        .run_provider
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            row.default_provider
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or("");
    let agent_label = format_provider_harness(provider);

    let model = run_payload
        .pointer("/modelSelection/model")
        .or_else(|| thread_payload.pointer("/modelSelection/model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    let workspace_path = thread_payload
        .get("worktreePath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            row.workspace_root
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .map(str::to_owned);

    let project = row
        .project_title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| workspace_path.as_deref().and_then(project_name));

    let title = {
        let trimmed = row.title.trim();
        if !trimmed.is_empty() {
            trimmed.to_owned()
        } else {
            project
                .clone()
                .unwrap_or_else(|| row.thread_id.chars().take(8).collect())
        }
    };

    let run_status = row
        .run_status
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    let has_pending_request = row.pending_request_id.is_some();
    let (state, priority) = if has_pending_request {
        ("waiting", TASK_PRIORITY_ACTIVE)
    } else if matches!(run_status.as_str(), "running" | "starting" | "in_progress") {
        ("running", TASK_PRIORITY_ACTIVE)
    } else if matches!(run_status.as_str(), "failed" | "error") {
        (
            "error",
            if settled {
                TASK_PRIORITY_SETTLED
            } else {
                TASK_PRIORITY_ACTIVE
            },
        )
    } else if run_status == "completed" {
        if settled {
            // Settled threads render as calm queued/idle cards and sit at the
            // lowest priority tier so their slots are reused first when active
            // or fresher tasks need a button.
            ("queued", TASK_PRIORITY_SETTLED)
        } else {
            ("completed", TASK_PRIORITY_IDLE)
        }
    } else {
        (
            "queued",
            if settled {
                TASK_PRIORITY_SETTLED
            } else {
                TASK_PRIORITY_IDLE
            },
        )
    };

    let started_at_ms = run_payload
        .get("startedAt")
        .and_then(Value::as_str)
        .and_then(parse_iso8601_ms)
        .or_else(|| {
            row.run_requested_at
                .as_deref()
                .and_then(parse_iso8601_ms)
        });
    let finished_at_ms = run_payload
        .get("completedAt")
        .and_then(Value::as_str)
        .and_then(parse_iso8601_ms)
        .or_else(|| {
            row.run_completed_at
                .as_deref()
                .and_then(parse_iso8601_ms)
        });

    let updated_at_ms = [
        parse_iso8601_ms(&row.updated_at),
        row.pending_request_created_at
            .as_deref()
            .and_then(parse_iso8601_ms),
        finished_at_ms,
        started_at_ms,
    ]
    .into_iter()
    .flatten()
    .max();

    let task_id = format!("t3:{}", row.thread_id);
    let mut context = json!({
        "project": project,
        "workspace_path": workspace_path,
        "task": title,
        "model": model,
        "status": state,
        "task_id": task_id,
    });
    if has_pending_request {
        let wait_reason = row
            .pending_request_kind
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("approval");
        let prompt = row
            .pending_request_payload
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|payload| extract_request_prompt(&payload))
            .unwrap_or_else(|| format!("T3 Code waiting for {wait_reason}"));
        context["wait_reason"] = json!(wait_reason);
        context["prompt"] = json!(prompt);
    }

    let mut task = json!({
        "task_id": task_id,
        "provider": (!provider.is_empty()).then_some(provider),
        "agent_label": agent_label,
        "title": title,
        "project": project,
        "workspace_path": workspace_path,
        "model": model,
        "state": state,
        "priority": priority,
        "timing_authoritative": true,
        "started_at_ms": (state != "queued").then_some(started_at_ms).flatten(),
        "finished_at_ms": (state == "completed").then_some(finished_at_ms).flatten(),
        "context": context,
    });
    if let Some(updated_at_ms) = updated_at_ms {
        task["updated_at_ms"] = json!(updated_at_ms);
    }
    task
}

fn extract_request_prompt(payload: &Value) -> Option<String> {
    for key in ["prompt", "question", "message", "title", "command", "summary"] {
        if let Some(text) = payload
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(text.chars().take(160).collect());
        }
    }
    None
}

fn project_name(path: &str) -> Option<String> {
    path.trim()
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
}

/// Reads the latest user prompt or assistant response for a T3 Code thread
/// directly from `statev2.sqlite` (read-only). Used by the CLI helper
/// `--read-t3-message <thread_id> user|assistant` invoked by the Stream Deck
/// plugin for `task.copy-prompt` and `task.copy-response`.
pub fn read_t3_last_message(thread_id: &str, role: &str) -> Result<String, String> {
    let db_path = t3_db_path().ok_or_else(|| "could not locate T3 Code home directory".to_owned())?;
    read_t3_last_message_from_path(&db_path, thread_id, role)
}

pub fn read_t3_last_message_from_path(
    db_path: &Path,
    thread_id: &str,
    role: &str,
) -> Result<String, String> {
    let role = role.trim();
    if role != "user" && role != "assistant" {
        return Err("role must be user or assistant".to_owned());
    }
    let bare_id = thread_id.trim().strip_prefix("t3:").unwrap_or(thread_id.trim());
    if !is_valid_t3_identifier(bare_id) {
        return Err("invalid T3 thread id".to_owned());
    }
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("could not open T3 database: {error}"))?;
    connection
        .busy_timeout(Duration::from_millis(100))
        .map_err(|error| format!("could not configure T3 database timeout: {error}"))?;

    read_t3_last_message_from_connection(&connection, bare_id, role)
}

fn read_t3_last_message_from_connection(
    connection: &Connection,
    bare_id: &str,
    role: &str,
) -> Result<String, String> {
    let mut statement = connection
        .prepare(
            "SELECT payload_json
             FROM orchestration_v2_projection_messages
             WHERE thread_id = ?1 AND role = ?2
             ORDER BY created_at DESC, rowid DESC
             LIMIT 20;",
        )
        .map_err(|error| format!("could not query T3 messages: {error}"))?;

    let rows = statement
        .query_map(rusqlite::params![bare_id, role], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("could not read T3 messages: {error}"))?;

    for raw in rows.flatten() {
        let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if let Some(text) = extract_message_text(&payload) {
            return Ok(text);
        }
    }
    Err(format!("no {role} message found for T3 thread {bare_id}"))
}

fn extract_message_text(payload: &Value) -> Option<String> {
    if let Some(text) = payload
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(text.to_owned());
    }
    if let Some(parts) = payload.get("parts").and_then(Value::as_array) {
        let joined = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !joined.is_empty() {
            return Some(joined);
        }
    }
    None
}

/// Parses an RFC 3339 / ISO-8601 UTC or offset timestamp (e.g.
/// `2026-11-25T20:43:53.123Z` or `2026-11-25T21:43:53+01:00`) into Unix
/// epoch milliseconds without external date crates.
pub fn parse_iso8601_ms(input: &str) -> Option<u64> {
    let s = input.trim();
    if s.len() < 19 {
        return None;
    }
    let bytes = s.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' || (bytes[10] != b'T' && bytes[10] != b' ') {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    let second: i64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }

    let mut rest = &s[19..];
    let mut millis: i64 = 0;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits_len = after_dot
            .bytes()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits_len == 0 {
            return None;
        }
        let frac = &after_dot[..digits_len];
        let mut scaled = String::with_capacity(3);
        for ch in frac.chars().take(3) {
            scaled.push(ch);
        }
        while scaled.len() < 3 {
            scaled.push('0');
        }
        millis = scaled.parse().ok()?;
        rest = &after_dot[digits_len..];
    }

    let offset_seconds: i64 = if rest.is_empty() || rest == "Z" || rest == "z" {
        0
    } else if rest.len() >= 6 && (rest.starts_with('+') || rest.starts_with('-')) {
        let sign: i64 = if rest.starts_with('+') { 1 } else { -1 };
        let tz_hour: i64 = rest.get(1..3)?.parse().ok()?;
        let tz_min: i64 = if rest.as_bytes().get(3) == Some(&b':') {
            rest.get(4..6)?.parse().ok()?
        } else {
            rest.get(3..5)?.parse().ok()?
        };
        sign * (tz_hour * 3600 + tz_min * 60)
    } else {
        return None;
    };

    let days = days_from_civil(year, month, day);
    let total_seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    if total_seconds < 0 {
        return None;
    }
    u64::try_from(total_seconds)
        .ok()?
        .checked_mul(1000)?
        .checked_add(millis as u64)
}

/// Howard Hinnant's civil-to-days algorithm (days since 1970-01-01).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let d = i64::from(day);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projection_projects (
                project_id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                workspace_root TEXT NOT NULL
            );
            CREATE TABLE orchestration_v2_projection_threads (
                thread_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                title TEXT NOT NULL,
                default_provider TEXT,
                provider_instance_id TEXT,
                runtime_mode TEXT,
                interaction_mode TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                archived_at TEXT,
                deleted_at TEXT,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE orchestration_v2_projection_runs (
                run_id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                provider TEXT,
                provider_instance_id TEXT,
                status TEXT NOT NULL,
                requested_at TEXT NOT NULL,
                completed_at TEXT,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE orchestration_v2_projection_runtime_requests (
                runtime_request_id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                node_id TEXT,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                resolved_at TEXT,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE orchestration_v2_projection_messages (
                message_id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                role TEXT NOT NULL,
                streaming INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                payload_json TEXT NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn formats_provider_harnesses() {
        assert_eq!(format_provider_harness("codex"), "T3-Codex");
        assert_eq!(format_provider_harness("claudeAgent"), "T3-Claude");
        assert_eq!(
            format_provider_harness("acpRegistry_google_antigravity"),
            "T3-AGY"
        );
        assert_eq!(format_provider_harness("acpRegistry_devin"), "T3-Devin");
        assert_eq!(format_provider_harness("acpRegistry_glm_agent"), "T3-GLM");
        assert_eq!(format_provider_harness("acpRegistry_custom_bot"), "T3-CustomBot");
        assert_eq!(format_provider_harness(""), "T3");
    }

    #[test]
    fn parses_iso8601_timestamps() {
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:01.250Z"), Some(1250));
        assert_eq!(
            parse_iso8601_ms("2026-04-15T12:30:45.500Z"),
            Some(1_776_256_245_500)
        );
        assert_eq!(
            parse_iso8601_ms("2026-04-15T13:30:45.500+01:00"),
            Some(1_776_256_245_500)
        );
        assert_eq!(parse_iso8601_ms("not-a-date"), None);
    }

    #[test]
    fn queued_follow_up_keeps_the_active_run_blue() {
        let conn = create_test_db();
        conn.execute_batch(
            r#"INSERT INTO orchestration_v2_projection_threads VALUES
             ('thr-queue', 'p1', 'Queue a follow-up', 'codex', NULL, NULL, NULL,
              '2026-04-15T10:00:00Z', '2026-04-15T10:06:00Z', NULL, NULL, '{}');
             INSERT INTO orchestration_v2_projection_runs VALUES
             ('run-active', 'thr-queue', 1, 'codex', NULL, 'running',
              '2026-04-15T10:00:00Z', NULL,
              '{"startedAt":"2026-04-15T10:01:00Z","modelSelection":{"model":"active-model"}}'),
             ('run-queued', 'thr-queue', 2, 'claudeAgent', NULL, 'queued',
              '2026-04-15T10:06:00Z', NULL, '{}');"#,
        )
        .unwrap();

        let mut board = crate::tasks::TaskBoard::new();
        board.set_device("deck", 1, true);
        let snapshot = read_t3_snapshot_from_connection(&conn, 1).unwrap();
        board
            .publish_tasks(
                crate::daemon::T3_POLL_SESSION,
                crate::routing::AgentId::T3,
                &snapshot,
                1,
            )
            .unwrap();

        let card = &board.rendered_slots("deck", 1)[0];
        assert_eq!(card["status"], "running");
        assert_eq!(card["c"], 0x1565c0);
        assert_eq!(card["provider"], "codex");
        assert_eq!(card["model"], "active-model");
        assert_eq!(
            card["started_at_ms"].as_u64(),
            parse_iso8601_ms("2026-04-15T10:01:00Z")
        );

        conn.execute(
            "UPDATE orchestration_v2_projection_runs
             SET status = 'completed', completed_at = '2026-04-15T10:07:00Z'
             WHERE run_id = 'run-active'",
            [],
        )
        .unwrap();
        let snapshot = read_t3_snapshot_from_connection(&conn, 1).unwrap();
        assert_eq!(snapshot["tasks"][0]["state"], "queued");
        assert!(snapshot["tasks"][0]["started_at_ms"].is_null());
    }

    #[test]
    fn snapshot_filters_imports_children_and_archived_and_ranks_settled_lowest() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO projection_projects VALUES ('p1', 'micro-emu', 'D:\\programming\\micro-emu')",
            [],
        )
        .unwrap();

        // 1. Running Antigravity thread
        conn.execute(
            "INSERT INTO orchestration_v2_projection_threads VALUES
             ('thr-running', 'p1', 'Add T3 support', 'acpRegistry_google_antigravity', NULL, NULL, NULL,
              '2026-04-15T10:00:00Z', '2026-04-15T10:05:00Z', NULL, NULL,
              '{\"modelSelection\":{\"model\":\"gemini-4-neon\"}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO orchestration_v2_projection_runs VALUES
             ('run-1', 'thr-running', 1, 'acpRegistry_google_antigravity', NULL, 'running',
              '2026-04-15T10:04:00Z', NULL,
              '{\"startedAt\":\"2026-04-15T10:04:05.000Z\",\"modelSelection\":{\"model\":\"gemini-4-neon\"}}')",
            [],
        )
        .unwrap();

        // 2. Waiting Codex thread (pending approval)
        conn.execute(
            "INSERT INTO orchestration_v2_projection_threads VALUES
             ('thr-waiting', 'p1', 'Deploy PR', 'codex', NULL, NULL, NULL,
              '2026-04-15T09:00:00Z', '2026-04-15T09:05:00Z', NULL, NULL,
              '{\"worktreePath\":\"D:\\\\worktrees\\\\deploy\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO orchestration_v2_projection_runtime_requests VALUES
             ('req-1', 'thr-waiting', NULL, 'approval', 'pending',
              '2026-04-15T09:06:00Z', NULL,
              '{\"prompt\":\"Allow git push?\"}')",
            [],
        )
        .unwrap();

        // 3. Completed unsettled Claude thread
        conn.execute(
            "INSERT INTO orchestration_v2_projection_threads VALUES
             ('thr-done', 'p1', 'Fix bug', 'claudeAgent', NULL, NULL, NULL,
              '2026-04-15T08:00:00Z', '2026-04-15T08:10:00Z', NULL, NULL, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO orchestration_v2_projection_runs VALUES
             ('run-2', 'thr-done', 1, 'claudeAgent', NULL, 'completed',
              '2026-04-15T08:01:00Z', '2026-04-15T08:10:00Z', '{}')",
            [],
        )
        .unwrap();

        // 4. Settled thread (should appear at TASK_PRIORITY_SETTLED as queued)
        conn.execute(
            "INSERT INTO orchestration_v2_projection_threads VALUES
             ('thr-settled', 'p1', 'Old task', 'codex', NULL, NULL, NULL,
              '2026-04-15T07:00:00Z', '2026-04-15T07:10:00Z', NULL, NULL,
              '{\"settledAt\":\"2026-04-15T07:15:00Z\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO orchestration_v2_projection_runs VALUES
             ('run-3', 'thr-settled', 1, 'codex', NULL, 'completed',
              '2026-04-15T07:01:00Z', '2026-04-15T07:10:00Z', '{}')",
            [],
        )
        .unwrap();

        // 5. Imported, archived, deleted, and child threads (must be excluded)
        conn.execute(
            "INSERT INTO orchestration_v2_projection_threads VALUES
             ('import:codex:abc', 'p1', 'Imported', 'codex', NULL, NULL, NULL,
              '2026-04-15T12:00:00Z', '2026-04-15T12:00:00Z', NULL, NULL, '{}'),
             ('thr-child', 'p1', 'Subagent', 'codex', NULL, NULL, NULL,
              '2026-04-15T12:00:00Z', '2026-04-15T12:00:00Z', NULL, NULL,
              '{\"lineage\":{\"parentThreadId\":\"thr-running\"}}'),
             ('thr-archived', 'p1', 'Archived', 'codex', NULL, NULL, NULL,
              '2026-04-15T12:00:00Z', '2026-04-15T12:00:00Z', '2026-04-15T12:01:00Z', NULL, '{}'),
             ('thr-deleted', 'p1', 'Deleted', 'codex', NULL, NULL, NULL,
              '2026-04-15T12:00:00Z', '2026-04-15T12:00:00Z', NULL, '2026-04-15T12:01:00Z', '{}')",
            [],
        )
        .unwrap();

        let snapshot = read_t3_snapshot_from_connection(&conn, 10).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 4);

        assert_eq!(tasks[0]["task_id"], "t3:thr-waiting");
        assert_eq!(tasks[0]["state"], "waiting");
        assert_eq!(tasks[0]["agent_label"], "T3-Codex");
        assert_eq!(tasks[0]["provider"], "codex");
        assert_eq!(tasks[0]["workspace_path"], "D:\\worktrees\\deploy");
        assert_eq!(tasks[0]["context"]["prompt"], "Allow git push?");

        assert_eq!(tasks[1]["task_id"], "t3:thr-running");
        assert_eq!(tasks[1]["state"], "running");
        assert_eq!(tasks[1]["agent_label"], "T3-AGY");
        assert_eq!(tasks[1]["model"], "gemini-4-neon");
        assert_eq!(tasks[1]["priority"], TASK_PRIORITY_ACTIVE);

        assert_eq!(tasks[2]["task_id"], "t3:thr-done");
        assert_eq!(tasks[2]["state"], "completed");
        assert_eq!(tasks[2]["agent_label"], "T3-Claude");
        assert_eq!(tasks[2]["priority"], TASK_PRIORITY_IDLE);

        assert_eq!(tasks[3]["task_id"], "t3:thr-settled");
        assert_eq!(tasks[3]["state"], "queued");
        assert_eq!(tasks[3]["priority"], TASK_PRIORITY_SETTLED);
    }

    #[test]
    fn reads_latest_user_and_assistant_messages() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO orchestration_v2_projection_messages VALUES
             ('m1', 'thr-1', 'user', 0, '2026-04-15T10:00:00Z', '{\"text\":\"first prompt\"}'),
             ('m2', 'thr-1', 'assistant', 0, '2026-04-15T10:01:00Z', '{\"text\":\"first reply\"}'),
             ('m3', 'thr-1', 'user', 0, '2026-04-15T10:02:00Z', '{\"text\":\"second prompt\"}'),
             ('m4', 'thr-1', 'assistant', 0, '2026-04-15T10:03:00Z', '{\"parts\":[{\"text\":\"part reply\"}]}')",
            [],
        )
        .unwrap();

        assert_eq!(
            read_t3_last_message_from_connection(&conn, "thr-1", "user").unwrap(),
            "second prompt"
        );
        assert_eq!(
            read_t3_last_message_from_connection(&conn, "thr-1", "assistant").unwrap(),
            "part reply"
        );
        assert!(read_t3_last_message_from_connection(&conn, "missing", "user").is_err());
    }
}
