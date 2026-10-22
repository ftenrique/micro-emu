//! Read-only access to Claude Code's live session registry.
//!
//! Every running Claude Code process (the CLI and the sessions hosted by the
//! Claude desktop app's Code tab) keeps a small JSON record in
//! `~/.claude/sessions/<pid>.json` with its name, working directory and a
//! `busy`/`idle` status. The daemon mirrors those records onto the shared
//! task board, the same way it mirrors ZCode's and Hermes' databases. The
//! bridge never writes to these files; a missing directory or an unreadable
//! record simply yields fewer cards.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::tasks::{TASK_PRIORITY_ACTIVE, TASK_PRIORITY_IDLE};

/// Only the end of a transcript is read when looking for the session model.
const TRANSCRIPT_TAIL_BYTES: u64 = 64 * 1024;

/// Records larger than this are not session registry entries.
const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// Root of Claude Code's per-user state (`CLAUDE_CONFIG_DIR` or `~/.claude`).
pub fn claude_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|home| !home.is_empty())?;
    Some(Path::new(&home).join(".claude"))
}

/// Reads the live Claude Code sessions as a `publish_tasks` snapshot holding
/// at most `max_tasks` cards. Returns `None` when the registry is missing.
pub fn read_claude_snapshot(max_tasks: usize) -> Option<Value> {
    let home = claude_home()?;
    read_claude_snapshot_from(&home, max_tasks, process_alive)
}

fn read_claude_snapshot_from(
    home: &Path,
    max_tasks: usize,
    alive: impl Fn(u32, Option<&str>) -> bool,
) -> Option<Value> {
    let entries = std::fs::read_dir(home.join("sessions")).ok()?;
    if max_tasks == 0 {
        return Some(json!({"tasks": []}));
    }
    let mut records = entries
        .flatten()
        .filter(|entry| {
            entry.path().extension().and_then(|ext| ext.to_str()) == Some("json")
                && entry
                    .metadata()
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_RECORD_BYTES)
        })
        .filter_map(|entry| {
            let record: Value = serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()?;
            let pid = u32::try_from(record.get("pid")?.as_u64()?).ok()?;
            let interactive = record
                .get("kind")
                .and_then(Value::as_str)
                .is_none_or(|kind| kind == "interactive");
            // A crashed process leaves its record behind; the creation time
            // guards against a recycled pid that now belongs to another program.
            (interactive && alive(pid, record.get("procStart").and_then(Value::as_str)))
                .then_some(record)
        })
        .collect::<Vec<_>>();
    // Active turns first, then the most recently touched sessions.
    records.sort_by(|left, right| {
        session_state(right)
            .is_active()
            .cmp(&session_state(left).is_active())
            .then_with(|| record_time(right, "updatedAt").cmp(&record_time(left, "updatedAt")))
    });
    let tasks = records
        .iter()
        .take(max_tasks)
        .filter_map(|record| {
            let model = record
                .get("sessionId")
                .and_then(Value::as_str)
                .and_then(|id| transcript_model(home, id));
            build_task(record, model)
        })
        .collect::<Vec<_>>();
    Some(json!({"tasks": tasks}))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionState {
    Running,
    Waiting,
    Error,
    Completed,
    Queued,
}

impl SessionState {
    fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Waiting)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Error => "error",
            Self::Completed => "completed",
            Self::Queued => "queued",
        }
    }
}

fn session_state(record: &Value) -> SessionState {
    let status = record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    match status.as_str() {
        "busy" | "running" | "working" | "thinking" | "streaming" => SessionState::Running,
        "waiting" | "waiting_for_input" | "needs_input" | "awaiting_input" | "permission"
        | "permission_request" | "awaiting_permission" | "needs_approval"
        | "approval_required" | "pending_approval" => SessionState::Waiting,
        "error" | "failed" | "crashed" => SessionState::Error,
        // An idle session either finished a turn or never ran one. A fresh
        // session reports idle within moments of starting.
        "idle" | "done" | "completed" => {
            let started = record_time(record, "startedAt");
            let changed = record_time(record, "statusUpdatedAt");
            if changed > started.saturating_add(5_000) {
                SessionState::Completed
            } else {
                SessionState::Queued
            }
        }
        _ => SessionState::Queued,
    }
}

fn record_time(record: &Value, key: &str) -> u64 {
    record.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn build_task(record: &Value, model: Option<String>) -> Option<Value> {
    let session_id = record
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())?;
    // `/clear` starts a new transcript (and session id) inside the same desktop
    // session; the host id keeps the card on its slot across that change.
    let stable_id = record
        .get("hostSessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .unwrap_or(session_id);
    let task_id = format!("claude:{stable_id}");
    let cwd = record
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.trim().is_empty());
    let project = cwd.map(project_name);
    let title = record
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| project.clone())
        .unwrap_or_else(|| session_id.chars().take(8).collect());
    let state = session_state(record);
    let priority = if state.is_active() {
        TASK_PRIORITY_ACTIVE
    } else {
        TASK_PRIORITY_IDLE
    };
    let mut task = json!({
        "task_id": task_id,
        "title": title,
        "project": project,
        "workspace_path": cwd,
        "model": model,
        "state": state.as_str(),
        "priority": priority,
        "context": {
            "project": project,
            "task": title,
            "model": model,
            "status": state.as_str(),
            "task_id": task_id,
        }
    });
    // A busy record's status timestamp is the exact start of the running turn.
    // Idle records carry no turn start, so completion timing is left to the
    // board, which keeps the observed start and stamps the finish.
    if state.is_active() {
        let started = record_time(record, "statusUpdatedAt");
        if started > 0 {
            task["started_at_ms"] = json!(started);
            task["finished_at_ms"] = Value::Null;
            task["timing_authoritative"] = json!(true);
        }
    }
    Some(task)
}

/// Finds the model of the latest assistant reply in the session transcript
/// (`~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`), reading only the
/// end of the file.
fn transcript_model(home: &Path, session_id: &str) -> Option<String> {
    if session_id.contains(['/', '\\']) || session_id.contains("..") {
        return None;
    }
    let file_name = format!("{session_id}.jsonl");
    let transcript = std::fs::read_dir(home.join("projects"))
        .ok()?
        .flatten()
        .map(|project| project.path().join(&file_name))
        .find(|path| path.is_file())?;
    let tail = read_tail(&transcript, TRANSCRIPT_TAIL_BYTES)?;
    tail.lines().rev().find_map(|line| {
        let entry: Value = serde_json::from_str(line).ok()?;
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        entry
            .get("message")?
            .get("model")?
            .as_str()
            .filter(|model| !model.is_empty() && !model.starts_with('<'))
            .map(str::to_owned)
    })
}

fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((length - start) as usize);
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // The first line is usually cut by the seek; skip it.
    Some(if start > 0 {
        text.split_once('\n').map(|(_, rest)| rest.to_owned()).unwrap_or_default()
    } else {
        text.into_owned()
    })
}

fn project_name(directory: &str) -> String {
    let trimmed = directory.trim_end_matches(['\\', '/']);
    trimmed
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(directory)
        .to_owned()
}

#[cfg(windows)]
fn process_alive(pid: u32, proc_start: Option<&str>) -> bool {
    type Handle = isize;
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn GetProcessTimes(
            process: Handle,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process == 0 {
        return false;
    }
    let mut exit_code = 0;
    let running = unsafe { GetExitCodeProcess(process, &mut exit_code) } != 0
        && exit_code == STILL_ACTIVE;
    let mut times = [FileTime::default(); 4];
    let [creation, exit, kernel, user] = &mut times;
    let timed = unsafe { GetProcessTimes(process, creation, exit, kernel, user) } != 0;
    unsafe {
        CloseHandle(process);
    }
    if !running {
        return false;
    }
    let expected = proc_start.and_then(|value| value.trim().parse::<u64>().ok());
    match (expected, timed) {
        (Some(expected), true) => {
            (u64::from(times[0].high) << 32 | u64::from(times[0].low)) == expected
        }
        _ => true,
    }
}

#[cfg(not(windows))]
fn process_alive(pid: u32, _proc_start: Option<&str>) -> bool {
    Path::new("/proc").join(pid.to_string()).exists() || !Path::new("/proc").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claude-home-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        dir
    }

    fn write_record(home: &Path, pid: u32, record: Value) {
        std::fs::write(home.join("sessions").join(format!("{pid}.json")), record.to_string())
            .unwrap();
    }

    fn record(pid: u32, id: &str, name: &str, status: &str, updated: u64) -> Value {
        json!({
            "pid": pid,
            "sessionId": id,
            "cwd": "D:\\Programming\\micro-emu",
            "startedAt": 1_000,
            "procStart": "42",
            "kind": "interactive",
            "entrypoint": "claude-desktop",
            "hostSessionId": format!("local_{id}"),
            "name": name,
            "status": status,
            "updatedAt": updated,
            "statusUpdatedAt": updated,
        })
    }

    #[test]
    fn maps_busy_and_idle_sessions_to_running_and_completed_cards() {
        let home = temp_home("states");
        write_record(&home, 1, record(1, "a", "Idle work", "idle", 90_000));
        write_record(&home, 2, record(2, "b", "Busy work", "busy", 50_000));
        let snapshot = read_claude_snapshot_from(&home, 3, |_, _| true).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 2);
        // Active sessions sort first even when older.
        assert_eq!(tasks[0]["task_id"], "claude:local_b");
        assert_eq!(tasks[0]["state"], "running");
        assert_eq!(tasks[0]["priority"], TASK_PRIORITY_ACTIVE);
        assert_eq!(tasks[0]["started_at_ms"], 50_000);
        assert_eq!(tasks[0]["timing_authoritative"], true);
        assert_eq!(tasks[0]["project"], "micro-emu");
        assert_eq!(tasks[1]["title"], "Idle work");
        assert_eq!(tasks[1]["state"], "completed");
        assert!(tasks[1].get("timing_authoritative").is_none());
    }

    #[test]
    fn fresh_idle_session_is_queued_not_completed() {
        let home = temp_home("fresh");
        write_record(&home, 1, record(1, "a", "New", "idle", 1_500));
        let snapshot = read_claude_snapshot_from(&home, 3, |_, _| true).unwrap();
        assert_eq!(snapshot["tasks"][0]["state"], "queued");
    }

    #[test]
    fn dead_and_non_interactive_sessions_are_skipped_and_limit_is_honored() {
        let home = temp_home("filter");
        write_record(&home, 1, record(1, "dead", "Dead", "busy", 10_000));
        let mut print = record(2, "print", "Print", "busy", 10_000);
        print["kind"] = json!("print");
        write_record(&home, 2, print);
        write_record(&home, 3, record(3, "old", "Old", "idle", 20_000));
        write_record(&home, 4, record(4, "new", "New", "idle", 30_000));
        let snapshot = read_claude_snapshot_from(&home, 1, |pid, _| pid != 1).unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "claude:local_new");
    }

    #[test]
    fn missing_registry_disables_the_feed() {
        let home = std::env::temp_dir().join(format!("claude-home-{}-none", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        assert!(read_claude_snapshot_from(&home, 3, |_, _| true).is_none());
    }

    #[test]
    fn untitled_session_falls_back_to_project_name() {
        let mut entry = record(1, "a", "", "idle", 90_000);
        entry.as_object_mut().unwrap().remove("hostSessionId");
        let task = build_task(&entry, None).unwrap();
        assert_eq!(task["title"], "micro-emu");
        assert_eq!(task["task_id"], "claude:a");
    }

    #[test]
    fn reads_model_from_the_latest_assistant_reply() {
        let home = temp_home("model");
        let project = home.join("projects").join("D--Programming-micro-emu");
        std::fs::create_dir_all(&project).unwrap();
        let lines = [
            json!({"type":"assistant","message":{"model":"claude-old"}}),
            json!({"type":"user","message":{"content":"hi"}}),
            json!({"type":"assistant","message":{"model":"claude-opus-5-5"}}),
            json!({"type":"assistant","message":{"model":"<synthetic>"}}),
        ];
        let body = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
        std::fs::write(project.join("sess-1.jsonl"), body).unwrap();
        assert_eq!(transcript_model(&home, "sess-1").as_deref(), Some("claude-opus-5-5"));
        assert_eq!(transcript_model(&home, "../escape"), None);
        assert_eq!(transcript_model(&home, "missing"), None);
    }
}
