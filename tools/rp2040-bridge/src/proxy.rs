//! STDIO-to-TCP proxy: each agent (Codex CLI, Hermes Desktop) launches a
//! proxy as its MCP stdio server. The proxy connects to the daemon over TCP
//! loopback, sends a hello line identifying the agent, and pumps lines
//! bidirectionally. With `--autostart` it spawns the daemon if it is not
//! already running.

use crate::routing::AgentId;
use serde_json::json;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn log_context() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    format!("ts={millis} pid={}", std::process::id())
}

pub struct ProxyOptions {
    pub connect: String,
    pub agent: AgentId,
    pub autostart: bool,
    pub daemon_args: Vec<String>,
    pub exe: String,
}

/// Runs the STDIO<->TCP proxy. Returns when stdin closes or the daemon
/// connection is permanently lost.
pub fn run_proxy(options: ProxyOptions) -> Result<(), String> {
    eprintln!(
        "proxy [{}] starting agent={} connect={} autostart={} daemon_args={:?}",
        log_context(),
        options.agent.as_str(),
        options.connect,
        options.autostart,
        options.daemon_args
    );
    let stream = connect_with_retry(&options)?;
    eprintln!(
        "proxy [{}] connected to daemon at {}",
        log_context(),
        options.connect
    );
    let read_stream = stream
        .try_clone()
        .map_err(|error| format!("could not clone proxy socket: {error}"))?;
    let mut write_stream = stream;

    // Send the hello line identifying the agent.
    let instance_id = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let hello = json!({"bridge": "hello", "version": 1, "agent": options.agent.as_str(), "instance_id": instance_id, "capabilities": {"focus": true}});
    let hello_line = serde_json::to_string(&hello).map_err(|e| e.to_string())?;
    write_stream
        .write_all(hello_line.as_bytes())
        .and_then(|_| write_stream.write_all(b"\n"))
        .and_then(|_| write_stream.flush())
        .map_err(|error| format!("failed to send hello: {error}"))?;
    eprintln!(
        "proxy [{}] sent bridge hello instance={}",
        log_context(),
        instance_id
    );

    // Channel for lines read from the daemon.
    let (daemon_tx, daemon_rx) = mpsc::channel::<Result<String, String>>();

    // Daemon reader thread.
    thread::Builder::new()
        .name("proxy-daemon-reader".to_owned())
        .spawn(move || {
            let reader = BufReader::new(read_stream);
            for line in reader.lines() {
                match line {
                    Ok(line) if line.trim().is_empty() => continue,
                    Ok(line) => {
                        if daemon_tx.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = daemon_tx.send(Err(format!("daemon read failed: {error}")));
                        return;
                    }
                }
            }
            let _ = daemon_tx.send(Err("daemon closed connection".to_owned()));
        })
        .map_err(|error| format!("failed to start daemon reader: {error}"))?;

    // Stdin reader thread.
    let (stdin_tx, stdin_rx) = mpsc::channel::<Result<String, String>>();
    thread::Builder::new()
        .name("proxy-stdin".to_owned())
        .spawn(move || {
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(line) if line.trim().is_empty() => continue,
                    Ok(line) => {
                        if stdin_tx.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = stdin_tx.send(Err(format!("stdin read failed: {error}")));
                        return;
                    }
                }
            }
            let _ = stdin_tx.send(Err("stdin closed".to_owned()));
        })
        .map_err(|error| format!("failed to start stdin reader: {error}"))?;

    let stdout = io::stdout();
    loop {
        // Check stdin.
        match stdin_rx.try_recv() {
            Ok(Ok(line)) => {
                if let Err(error) = write_stream
                    .write_all(line.as_bytes())
                    .and_then(|_| write_stream.write_all(b"\n"))
                    .and_then(|_| write_stream.flush())
                {
                    eprintln!("proxy: write to daemon failed: {error}");
                    return Ok(());
                }
            }
            Ok(Err(error)) if error == "stdin closed" => {
                eprintln!("proxy [{}] MCP stdin closed", log_context());
                return Ok(());
            }
            Ok(Err(error)) => {
                eprintln!("proxy [{}] stdin error: {error}", log_context());
                return Err(error);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
        }

        // Check daemon.
        match daemon_rx.try_recv() {
            Ok(Ok(line)) => {
                let mut stdout = stdout.lock();
                if let Err(error) = write_stream_to_stdout(&mut stdout, &line) {
                    eprintln!("proxy: write to stdout failed: {error}");
                    return Ok(());
                }
            }
            Ok(Err(error)) if error == "daemon closed connection" => {
                eprintln!("proxy [{}] daemon disconnected", log_context());
                // Respond to any pending requests with an error.
                let mut stdout = stdout.lock();
                let _ = write_stream_to_stdout(
                    &mut stdout,
                    &serde_json::to_string(&json!({
                        "jsonrpc": "2.0",
                        "error": {"code": -32001, "message": "bridge daemon unavailable"}
                    }))
                    .unwrap_or_default(),
                );
                return Ok(());
            }
            Ok(Err(error)) => {
                eprintln!("proxy [{}] daemon reader error: {error}", log_context());
                return Ok(());
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
        }

        thread::sleep(Duration::from_millis(5));
    }
}

fn write_stream_to_stdout(stdout: &mut io::StdoutLock, line: &str) -> Result<(), String> {
    stdout
        .write_all(line.as_bytes())
        .and_then(|_| stdout.write_all(b"\n"))
        .and_then(|_| stdout.flush())
        .map_err(|e| e.to_string())
}

fn connect_with_retry(options: &ProxyOptions) -> Result<TcpStream, String> {
    let mut delay = Duration::from_millis(200);
    let max_attempts = if options.autostart { 50 } else { 5 };
    let mut autostarted = false;
    for attempt in 0..max_attempts {
        match TcpStream::connect(&options.connect) {
            Ok(stream) => {
                eprintln!(
                    "proxy [{}] TCP connect succeeded attempt={} address={}",
                    log_context(),
                    attempt + 1,
                    options.connect
                );
                return Ok(stream);
            }
            Err(error) => {
                if options.autostart && !autostarted && attempt > 0 {
                    eprintln!(
                        "proxy [{}] daemon not reachable at {} ({error}); autostarting",
                        log_context(),
                        options.connect
                    );
                    autostart_daemon(options)?;
                    autostarted = true;
                } else if attempt > 0 {
                    eprintln!(
                        "proxy [{}] connect attempt {} to {} failed: {error}",
                        log_context(),
                        attempt + 1,
                        options.connect
                    );
                }
                thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(2));
            }
        }
    }
    Err(format!(
        "could not connect to daemon at {} after {max_attempts} attempts",
        options.connect
    ))
}

fn autostart_daemon(options: &ProxyOptions) -> Result<(), String> {
    let lockfile = daemon_lockfile_path();
    let lock_guard = lockfile.as_ref().and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        acquire_lock(path)
    });
    // A losing proxy must wait for the winner rather than spawning a second daemon.
    if lock_guard.is_none() {
        eprintln!(
            "proxy [{}] autostart lock held by another process; waiting",
            log_context()
        );
        return wait_for_daemon(&options.connect, Duration::from_secs(15));
    }
    if TcpStream::connect(&options.connect).is_ok() {
        eprintln!(
            "proxy [{}] daemon became reachable before spawn",
            log_context()
        );
        return Ok(());
    }
    let mut command = Command::new(&options.exe);
    command.arg("--daemon");
    for arg in &options.daemon_args {
        command.arg(arg);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let child = command
        .spawn()
        .map_err(|error| format!("failed to autostart daemon: {error}"))?;
    eprintln!(
        "proxy [{}] autostarted daemon pid={} exe={} args={:?}",
        log_context(),
        child.id(),
        options.exe,
        options.daemon_args
    );
    // Keep the lock alive until the listener is observable.
    let result = wait_for_daemon(&options.connect, Duration::from_secs(10));
    eprintln!("proxy [{}] daemon wait result={:?}", log_context(), result);
    result
}

fn wait_for_daemon(address: &str, timeout: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if TcpStream::connect(address).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!("daemon did not become reachable at {address}"))
}

fn daemon_lockfile_path() -> Option<PathBuf> {
    let local_app_data = std::env::var("LOCALAPPDATA").ok()?;
    Some(
        PathBuf::from(local_app_data)
            .join("micro-emu")
            .join("bridge-daemon.lock"),
    )
}

struct LockGuard {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        self.file.take();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn acquire_lock(path: &PathBuf) -> Option<LockGuard> {
    let open = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    };
    match open() {
        Ok(mut file) => {
            let _ = writeln!(file, "{}", std::process::id());
            Some(LockGuard {
                path: path.clone(),
                file: Some(file),
            })
        }
        Err(_) => {
            let stale = std::fs::metadata(path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(10));
            if stale {
                let _ = std::fs::remove_file(path);
                if let Ok(mut file) = open() {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Some(LockGuard {
                        path: path.clone(),
                        file: Some(file),
                    });
                }
            }
            None
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_guard_is_exclusive_and_releases() {
        let path = std::env::temp_dir().join(format!(
            "micro-emu-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let guard = acquire_lock(&path).expect("first lock should win");
        assert!(acquire_lock(&path).is_none());
        drop(guard);
        assert!(acquire_lock(&path).is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hello_line_identifies_agent() {
        let hello = json!({"bridge": "hello", "agent": "hermes"});
        assert_eq!(hello["agent"], "hermes");
        let hello = json!({"bridge": "hello", "agent": "codex"});
        assert_eq!(hello["agent"], "codex");
        let hello = json!({"bridge": "hello", "agent": "zcode"});
        assert_eq!(hello["agent"], "zcode");
    }
}
