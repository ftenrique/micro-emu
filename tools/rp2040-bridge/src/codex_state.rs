//! Authoritative Codex task snapshots derived from local desktop state.
//!
//! The HID `v.oai.thstatus` feed is a presentation protocol: its effects,
//! colours, and brightness can change when focus changes. Task identity,
//! metadata, lifecycle, and timing instead come from Codex's thread database
//! and append-only rollout events.

use crate::tasks::{
    CODEX_HID_SLOTS, TASK_PRIORITY_ACTIVE, TASK_PRIORITY_IDLE, TaskState,
};
use rusqlite::OpenFlags;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Lifecycle {
    state: Option<TaskState>,
    started_at_ms: Option<u128>,
    finished_at_ms: Option<u128>,
    /// The Codex turn whose lifecycle is currently reflected above. Keeping
    /// this identity prevents a delayed terminal event for an older turn from
    /// finishing a newer turn in the same task.
    active_turn_id: Option<String>,
    /// Escalated tool calls that have been issued but have not yet received a
    /// corresponding result. Codex records these separately from task
    /// lifecycle events, so they must be tracked independently.
    pending_approval_calls: HashMap<String, ()>,
}

#[derive(Clone, Debug, Default)]
struct RolloutCursor {
    path: PathBuf,
    offset: u64,
    lifecycle: Lifecycle,
    /// Some(true) only after this rollout's session_meta ID has matched the
    /// database thread that referenced it. A mismatched or stale path must
    /// never lend its lifecycle to another task.
    thread_id_verified: Option<bool>,
}

#[derive(Clone, Debug, Default)]
struct FocusCursor {
    path: PathBuf,
    offset: u64,
    selected_task_id: Option<String>,
}

#[derive(Default)]
struct CodexStateCache {
    rollouts: HashMap<String, RolloutCursor>,
    focus: FocusCursor,
}

pub fn read_codex_snapshot(max_tasks: usize) -> Option<Value> {
    static CACHE: OnceLock<Mutex<CodexStateCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(CodexStateCache::default()));
    let mut cache = cache.lock().ok()?;
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let logs_dir = std::env::var("LOCALAPPDATA")
        .ok()
        .map(|local| Path::new(&local).join("Codex").join("Logs"));
    read_codex_snapshot_from(
        &Path::new(&home).join(".codex"),
        logs_dir.as_deref(),
        max_tasks,
        &mut cache,
    )
}

fn read_codex_snapshot_from(
    codex_dir: &Path,
    logs_dir: Option<&Path>,
    max_tasks: usize,
    cache: &mut CodexStateCache,
) -> Option<Value> {
    let titles = read_index_titles(&codex_dir.join("session_index.jsonl"));
    let connection = rusqlite::Connection::open_with_flags(
        codex_dir.join("state_5.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    connection
        .busy_timeout(std::time::Duration::from_millis(100))
        .ok()?;

    let mut statement = connection
        .prepare(
            "SELECT id, COALESCE(NULLIF(name, ''), NULLIF(title, ''), \
             NULLIF(first_user_message, ''), id), NULLIF(cwd, ''), \
             NULLIF(model, ''), NULLIF(reasoning_effort, ''), rollout_path, \
             recency_at_ms, updated_at_ms \
             FROM threads \
             WHERE archived = 0 AND thread_source = 'user' \
             ORDER BY recency_at_ms DESC, updated_at_ms DESC \
             LIMIT ?1",
        )
        .ok()?;
    let rows = statement
        .query_map([u64::try_from(max_tasks).unwrap_or(u64::MAX)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, u64>(6)?,
                row.get::<_, u64>(7)?,
            ))
        })
        .ok()?;

    let mut tasks = Vec::new();
    for (source_slot, row) in rows.enumerate() {
        let (thread_id, fallback_title, cwd, model, effort, rollout_path, recency, updated) =
            row.ok()?;
        let lifecycle = refresh_rollout(cache, &thread_id, Path::new(&rollout_path));
        let title = titles.get(&thread_id).cloned().unwrap_or(fallback_title);
        // A native shell approval is not a lifecycle event. While it is open,
        // it must override the normal running/selected presentation so the
        // Stream Deck does not remain blue.
        let awaiting_approval = !lifecycle.pending_approval_calls.is_empty();
        let state = awaiting_approval
            .then_some(TaskState::Waiting)
            .or(lifecycle.state)
            .unwrap_or(TaskState::Queued);
        tasks.push(json!({
            "task_id": thread_id,
            "title": title,
            "project": cwd.as_deref().and_then(project_name_from_cwd),
            "model": model,
            "effort": effort,
            "state": state.as_str(),
            "priority": if state.active() || awaiting_approval {
                TASK_PRIORITY_ACTIVE
            } else {
                TASK_PRIORITY_IDLE
            },
            "source_slot": source_slot,
            // Only the six physical Micro positions have synthetic HID keys.
            // Extra Stream Deck task cards are selected through the desktop
            // task path instead of trying to emit nonexistent AG06/AG07 keys.
            "legacy_key": (source_slot < CODEX_HID_SLOTS)
                .then(|| format!("AG0{source_slot}")),
            "started_at_ms": lifecycle.started_at_ms,
            "finished_at_ms": lifecycle.finished_at_ms,
            "timing_authoritative": true,
            "recency_at_ms": recency,
            "updated_at_ms": updated
        }));
    }

    // An empty array is a valid publish: it clears stale Codex cards when
    // every thread is archived, instead of leaving them on the board.
    let selected_task_id = if tasks.is_empty() {
        None
    } else {
        logs_dir
            .and_then(|logs| refresh_focused_task(&mut cache.focus, logs))
            .filter(|selected| {
                tasks
                    .iter()
                    .any(|task| task.get("task_id").and_then(Value::as_str) == Some(selected))
            })
    };
    Some(json!({
        "selected_task_id": selected_task_id,
        "tasks": tasks
    }))
}

fn refresh_focused_task(cursor: &mut FocusCursor, logs_dir: &Path) -> Option<String> {
    let path = newest_primary_log(logs_dir)?;
    let metadata = std::fs::metadata(&path).ok()?;
    if cursor.path != path || metadata.len() < cursor.offset {
        *cursor = FocusCursor {
            path: path.clone(),
            ..FocusCursor::default()
        };
    }
    if metadata.len() == cursor.offset {
        return cursor.selected_task_id.clone();
    }

    let mut file = File::open(&path).ok()?;
    file.seek(SeekFrom::Start(cursor.offset)).ok()?;
    let mut reader = BufReader::new(file);
    loop {
        let line_start = cursor.offset;
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).ok()?;
        if bytes == 0 {
            break;
        }
        if !line.ends_with('\n') {
            cursor.offset = line_start;
            break;
        }
        cursor.offset = cursor.offset.saturating_add(bytes as u64);
        apply_focus_event(cursor, &line);
    }
    cursor.selected_task_id.clone()
}

fn newest_primary_log(logs_dir: &Path) -> Option<PathBuf> {
    let mut directory = logs_dir.to_owned();
    for _ in 0..3 {
        directory = std::fs::read_dir(&directory)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
            .max_by_key(|entry| entry.file_name())?
            .path();
    }
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().ok().is_some_and(|kind| kind.is_file())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.contains("-t0-") && name.ends_with(".log"))
        })
        .max_by_key(|entry| {
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
        })
        .map(|entry| entry.path())
}

fn apply_focus_event(cursor: &mut FocusCursor, line: &str) {
    if !line.contains("thread_stream_view_activity_changed")
        || !line.contains("rendererWindowAppearance=primary")
        || !line.contains("rendererWindowFocused=true")
        || !line.contains("rendererWindowVisible=true")
    {
        return;
    }
    let active = if line.contains(" active=true ") {
        true
    } else if line.contains(" active=false ") {
        false
    } else {
        return;
    };
    let Some(thread_id) = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("conversationId="))
        .filter(|id| !id.is_empty() && !id.starts_with("client-new-thread:"))
    else {
        return;
    };
    if active {
        cursor.selected_task_id = Some(thread_id.to_owned());
    } else if cursor.selected_task_id.as_deref() == Some(thread_id) {
        cursor.selected_task_id = None;
    }
}

fn read_index_titles(path: &Path) -> HashMap<String, String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let mut titles = HashMap::new();
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(title) = value
            .get("thread_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        else {
            continue;
        };
        titles.insert(id.to_owned(), title.to_owned());
    }
    titles
}

fn refresh_rollout(cache: &mut CodexStateCache, thread_id: &str, path: &Path) -> Lifecycle {
    let cursor = cache.rollouts.entry(thread_id.to_owned()).or_default();
    let Ok(metadata) = std::fs::metadata(path) else {
        return cursor.lifecycle.clone();
    };
    if cursor.path != path || metadata.len() < cursor.offset {
        *cursor = RolloutCursor {
            path: path.to_owned(),
            ..RolloutCursor::default()
        };
    }
    if metadata.len() == cursor.offset {
        return cursor.lifecycle.clone();
    }

    let Ok(mut file) = File::open(path) else {
        return cursor.lifecycle.clone();
    };
    if file.seek(SeekFrom::Start(cursor.offset)).is_err() {
        return cursor.lifecycle.clone();
    }
    let mut reader = BufReader::new(file);
    loop {
        let line_start = cursor.offset;
        let mut line = String::new();
        let Ok(bytes) = reader.read_line(&mut line) else {
            break;
        };
        if bytes == 0 {
            break;
        }
        if !line.ends_with('\n') {
            cursor.offset = line_start;
            break;
        }
        cursor.offset = cursor.offset.saturating_add(bytes as u64);
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            if let Some(rollout_thread_id) = rollout_thread_id(&value) {
                let matches = rollout_thread_id == thread_id;
                cursor.thread_id_verified = Some(matches);
                if !matches {
                    cursor.lifecycle = Lifecycle::default();
                }
            } else if cursor.thread_id_verified == Some(true) {
                apply_lifecycle_event(&mut cursor.lifecycle, &value);
            }
        }
    }
    cursor.lifecycle.clone()
}

fn rollout_thread_id(value: &Value) -> Option<&str> {
    (value.get("type").and_then(Value::as_str) == Some("session_meta"))
        .then(|| value.get("payload")?.get("id")?.as_str())
        .flatten()
}

fn apply_lifecycle_event(lifecycle: &mut Lifecycle, value: &Value) {
    let Some(payload) = value.get("payload").and_then(Value::as_object) else {
        return;
    };

    // Approval requests arrive as response items rather than `event_msg`
    // lifecycle notifications. Associate the result with its call id so the
    // orange waiting state clears immediately after a decision is made.
    if value.get("type").and_then(Value::as_str) == Some("response_item") {
        match payload.get("type").and_then(Value::as_str) {
            Some("custom_tool_call")
                if payload.get("input").and_then(Value::as_str).is_some_and(|input| {
                    input.contains("require_escalated")
                        || input.contains("sandbox_permissions") && input.contains("escalated")
                }) =>
            {
                if let Some(call_id) = payload.get("call_id").and_then(Value::as_str) {
                    lifecycle
                        .pending_approval_calls
                        .insert(call_id.to_owned(), ());
                }
            }
            Some("custom_tool_call_output") => {
                if let Some(call_id) = payload.get("call_id").and_then(Value::as_str) {
                    lifecycle.pending_approval_calls.remove(call_id);
                }
            }
            _ => {}
        }
        return;
    }

    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return;
    }
    let millis = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_u64)
            .map(|seconds| u128::from(seconds) * 1_000)
    };
    let turn_id = || {
        payload
            .get("turn_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
    };
    match payload.get("type").and_then(Value::as_str) {
        Some("task_started") => {
            lifecycle.state = Some(TaskState::Running);
            lifecycle.started_at_ms = millis("started_at");
            lifecycle.finished_at_ms = None;
            lifecycle.active_turn_id = turn_id().map(str::to_owned);
        }
        Some("task_complete") => {
            if terminal_event_is_stale(lifecycle, turn_id(), millis("started_at")) {
                return;
            }
            lifecycle.state = Some(TaskState::Completed);
            lifecycle.started_at_ms = millis("started_at").or(lifecycle.started_at_ms);
            lifecycle.finished_at_ms = millis("completed_at");
            lifecycle.active_turn_id = turn_id()
                .map(str::to_owned)
                .or_else(|| lifecycle.active_turn_id.clone());
        }
        Some("turn_aborted") => {
            if terminal_event_is_stale(lifecycle, turn_id(), millis("started_at")) {
                return;
            }
            lifecycle.state = Some(TaskState::Paused);
            lifecycle.started_at_ms = millis("started_at").or(lifecycle.started_at_ms);
            lifecycle.finished_at_ms = millis("completed_at");
            lifecycle.active_turn_id = turn_id()
                .map(str::to_owned)
                .or_else(|| lifecycle.active_turn_id.clone());
        }
        _ => {}
    }
}

fn terminal_event_is_stale(
    lifecycle: &Lifecycle,
    event_turn_id: Option<&str>,
    event_started_at_ms: Option<u128>,
) -> bool {
    lifecycle
        .active_turn_id
        .as_deref()
        .zip(event_turn_id)
        .is_some_and(|(active, event)| active != event)
        || lifecycle
            .started_at_ms
            .zip(event_started_at_ms)
            .is_some_and(|(active, event)| active != event)
}

fn project_name_from_cwd(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_events_supply_stable_start_and_finish_times() {
        let mut lifecycle = Lifecycle::default();
        apply_lifecycle_event(
            &mut lifecycle,
            &json!({"type":"event_msg","payload":{
                "type":"task_started","started_at":100
            }}),
        );
        assert_eq!(lifecycle.state, Some(TaskState::Running));
        assert_eq!(lifecycle.started_at_ms, Some(100_000));
        assert_eq!(lifecycle.finished_at_ms, None);

        apply_lifecycle_event(
            &mut lifecycle,
            &json!({"type":"event_msg","payload":{
                "type":"task_complete","started_at":100,"completed_at":145
            }}),
        );
        assert_eq!(lifecycle.state, Some(TaskState::Completed));
        assert_eq!(lifecycle.started_at_ms, Some(100_000));
        assert_eq!(lifecycle.finished_at_ms, Some(145_000));
    }

    #[test]
    fn stale_terminal_event_cannot_finish_a_newer_turn() {
        let mut lifecycle = Lifecycle::default();
        apply_lifecycle_event(
            &mut lifecycle,
            &json!({"type":"event_msg","payload":{
                "type":"task_started","turn_id":"turn-new","started_at":200
            }}),
        );
        apply_lifecycle_event(
            &mut lifecycle,
            &json!({"type":"event_msg","payload":{
                "type":"task_complete","turn_id":"turn-old",
                "started_at":100,"completed_at":250
            }}),
        );

        assert_eq!(lifecycle.state, Some(TaskState::Running));
        assert_eq!(lifecycle.started_at_ms, Some(200_000));
        assert_eq!(lifecycle.finished_at_ms, None);
        assert_eq!(lifecycle.active_turn_id.as_deref(), Some("turn-new"));
    }

    #[test]
    fn escalated_tool_call_overrides_lifecycle_until_its_result_arrives() {
        let mut lifecycle = Lifecycle {
            state: Some(TaskState::Running),
            ..Lifecycle::default()
        };
        apply_lifecycle_event(
            &mut lifecycle,
            &json!({"type":"response_item","payload":{
                "type":"custom_tool_call","call_id":"approval-1",
                "input":"tools.exec_command({ sandbox_permissions: require_escalated })"
            }}),
        );
        assert!(lifecycle.pending_approval_calls.contains_key("approval-1"));

        apply_lifecycle_event(
            &mut lifecycle,
            &json!({"type":"response_item","payload":{
                "type":"custom_tool_call_output","call_id":"approval-1","output":"approved"
            }}),
        );
        assert!(lifecycle.pending_approval_calls.is_empty());
        assert_eq!(lifecycle.state, Some(TaskState::Running));
    }

    #[test]
    fn rollout_lifecycle_is_rejected_when_thread_identity_does_not_match() {
        let root = std::env::temp_dir().join(format!(
            "micro-emu-codex-rollout-identity-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("rollout identity directory");
        let rollout = root.join("rollout.jsonl");
        std::fs::write(
            &rollout,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"thread-a"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"turn-a","started_at":100}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-a","started_at":100,"completed_at":145}}"#,
                "\n"
            ),
        )
        .expect("rollout");

        let mut cache = CodexStateCache::default();
        let wrong = refresh_rollout(&mut cache, "thread-b", &rollout);
        assert_eq!(wrong, Lifecycle::default());

        let matched = refresh_rollout(&mut cache, "thread-a", &rollout);
        assert_eq!(matched.state, Some(TaskState::Completed));
        assert_eq!(matched.started_at_ms, Some(100_000));
        assert_eq!(matched.finished_at_ms, Some(145_000));
        assert_eq!(matched.active_turn_id.as_deref(), Some("turn-a"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn selection_only_events_do_not_change_lifecycle() {
        let mut lifecycle = Lifecycle {
            state: Some(TaskState::Running),
            started_at_ms: Some(100_000),
            finished_at_ms: None,
            active_turn_id: None,
            pending_approval_calls: HashMap::new(),
        };
        apply_lifecycle_event(
            &mut lifecycle,
            &json!({"type":"event_msg","payload":{
                "type":"thread_settings_applied"
            }}),
        );
        assert_eq!(
            lifecycle,
            Lifecycle {
                state: Some(TaskState::Running),
                started_at_ms: Some(100_000),
                finished_at_ms: None,
                active_turn_id: None,
                pending_approval_calls: HashMap::new(),
            }
        );
    }

    #[test]
    fn focused_primary_task_wins_over_concurrent_activity() {
        let root =
            std::env::temp_dir().join(format!("micro-emu-codex-focus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let day = root.join("2026").join("08").join("11");
        std::fs::create_dir_all(&day).expect("focus log directory");
        let log = day.join("codex-desktop-test-t0-i1.log");
        let initial = concat!(
            "2026-08-11T10:00:00Z info thread_stream_view_activity_changed ",
            "active=true conversationId=thread-a rendererWindowAppearance=primary ",
            "rendererWindowFocused=true rendererWindowVisible=true\n",
            "2026-08-11T10:00:01Z info thread_stream_view_activity_changed ",
            "active=true conversationId=running-thread rendererWindowAppearance=avatarOverlay ",
            "rendererWindowFocused=false rendererWindowVisible=false\n",
        );
        std::fs::write(&log, initial).expect("initial focus log");
        let mut cursor = FocusCursor::default();
        assert_eq!(
            refresh_focused_task(&mut cursor, &root).as_deref(),
            Some("thread-a")
        );

        let switched = concat!(
            "2026-08-11T10:00:02Z info thread_stream_view_activity_changed ",
            "active=false conversationId=thread-a rendererWindowAppearance=primary ",
            "rendererWindowFocused=true rendererWindowVisible=true\n",
            "2026-08-11T10:00:02Z info thread_stream_view_activity_changed ",
            "active=true conversationId=thread-b rendererWindowAppearance=primary ",
            "rendererWindowFocused=true rendererWindowVisible=true\n",
        );
        std::fs::write(&log, format!("{initial}{switched}")).expect("switched focus log");
        assert_eq!(
            refresh_focused_task(&mut cursor, &root).as_deref(),
            Some("thread-b")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_starts_with_complete_thread_metadata_and_lifecycle() {
        let root =
            std::env::temp_dir().join(format!("micro-emu-codex-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temporary Codex directory");
        let rollout = root.join("rollout.jsonl");
        std::fs::write(
            &rollout,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"thread"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"task_started","started_at":100}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"task_complete","started_at":100,"completed_at":145}}"#,
                "\n"
            ),
        )
        .expect("rollout");
        std::fs::write(
            root.join("session_index.jsonl"),
            concat!(r#"{"id":"thread","thread_name":"Concise task"}"#, "\n"),
        )
        .expect("session index");

        let connection =
            rusqlite::Connection::open(root.join("state_5.sqlite")).expect("state database");
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY, title TEXT NOT NULL, name TEXT,
                    first_user_message TEXT NOT NULL DEFAULT '', cwd TEXT,
                    model TEXT, reasoning_effort TEXT, rollout_path TEXT NOT NULL,
                    archived INTEGER NOT NULL, thread_source TEXT,
                    recency_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
                );",
            )
            .expect("schema");
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, NULL, '', ?3, ?4, ?5, ?6, 0, 'user', 20, 20)",
                rusqlite::params![
                    "thread",
                    "Database title",
                    "D:/work/micro-emu",
                    "gpt-selected",
                    "xhigh",
                    rollout.to_string_lossy().as_ref()
                ],
            )
            .expect("thread");
        drop(connection);

        let logs = root.join("logs");
        let log_day = logs.join("2026").join("08").join("11");
        std::fs::create_dir_all(&log_day).expect("log directory");
        std::fs::write(
            log_day.join("codex-desktop-test-t0-i1.log"),
            concat!(
                "2026-08-11T10:00:00Z info thread_stream_view_activity_changed ",
                "active=true conversationId=thread rendererWindowAppearance=primary ",
                "rendererWindowFocused=true rendererWindowVisible=true\n"
            ),
        )
        .expect("focus log");

        let snapshot = read_codex_snapshot_from(
            &root,
            Some(&logs),
            crate::tasks::CODEX_TASK_SLOTS,
            &mut CodexStateCache::default(),
        )
        .expect("snapshot");
        assert_eq!(snapshot["selected_task_id"], "thread");
        let task = &snapshot["tasks"][0];
        assert_eq!(task["task_id"], "thread");
        assert_eq!(task["title"], "Concise task");
        assert_eq!(task["project"], "micro-emu");
        assert_eq!(task["model"], "gpt-selected");
        assert_eq!(task["effort"], "xhigh");
        assert_eq!(task["state"], "completed");
        assert_eq!(task["started_at_ms"], 100_000);
        assert_eq!(task["finished_at_ms"], 145_000);
        assert_eq!(task["source_slot"], 0);
        assert_eq!(task["legacy_key"], "AG00");

        let _ = std::fs::remove_dir_all(root);
    }
}
