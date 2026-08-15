//! Small append-only diagnostic log for daemon-side lifecycle events.
//!
//! The daemon is usually spawned by the Stream Deck plugin with its stderr
//! discarded, so `eprintln!` output is lost exactly when something needs
//! diagnosing. [`log`] mirrors concise one-line events into
//! `%LOCALAPPDATA%\micro-emu\logs\bridge-daemon.log`, rotating the previous
//! file once it exceeds [`MAX_BYTES`]. Logging is best-effort: a read-only
//! or missing profile directory must never affect the daemon loop.

use std::io::Write;
use std::path::PathBuf;

/// Size cap before the current log is rotated to `bridge-daemon.log.1`.
const MAX_BYTES: u64 = 512 * 1024;

/// Appends a timestamped line to the daemon diagnostic log.
pub fn log(message: &str) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    rotate_if_large(&path);
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = writeln!(file, "[{timestamp}] {message}");
}

fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    Some(
        base.join("micro-emu")
            .join("logs")
            .join("bridge-daemon.log"),
    )
}

fn rotate_if_large(path: &std::path::Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() <= MAX_BYTES {
        return;
    }
    let _ = std::fs::rename(path, path.with_extension("log.1"));
}

#[cfg(test)]
mod tests {
    use super::{MAX_BYTES, log_path};

    #[test]
    fn log_lives_under_localappdata_micro_emu() {
        let path = log_path().expect("LOCALAPPDATA is resolved on Windows CI");
        let text = path.to_string_lossy().to_lowercase();
        assert!(text.contains("micro-emu"));
        assert!(text.ends_with("bridge-daemon.log"));
    }

    #[test]
    fn rotation_cap_is_sane() {
        assert!(MAX_BYTES >= 64 * 1024);
    }
}
