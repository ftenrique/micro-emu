//! STDIO-to-TCP proxy: each agent (Codex CLI, Hermes Desktop) launches a
//! proxy as its MCP stdio server. The proxy connects to the daemon over TCP
//! loopback, sends a hello line identifying the agent, and pumps lines
//! bidirectionally. With `--autostart` it spawns the daemon if it is not
//! already running.

use crate::routing::AgentId;
use serde_json::json;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

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
    let stream = connect_with_retry(&options)?;
    let read_stream = stream
        .try_clone()
        .map_err(|error| format!("could not clone proxy socket: {error}"))?;
    let mut write_stream = stream;

    // Send the hello line identifying the agent.
    let hello = json!({"bridge": "hello", "agent": options.agent.as_str()});
    let hello_line = serde_json::to_string(&hello).map_err(|e| e.to_string())?;
    write_stream
        .write_all(hello_line.as_bytes())
        .and_then(|_| write_stream.write_all(b"\n"))
        .and_then(|_| write_stream.flush())
        .map_err(|error| format!("failed to send hello: {error}"))?;

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
            Ok(Err(error)) if error == "stdin closed" => return Ok(()),
            Ok(Err(error)) => return Err(error),
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
                eprintln!("proxy: daemon disconnected");
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
                eprintln!("proxy: {error}");
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
            Ok(stream) => return Ok(stream),
            Err(error) => {
                if options.autostart && !autostarted && attempt > 0 {
                    eprintln!(
                        "proxy: daemon not reachable at {} ({}); autostarting",
                        options.connect, error
                    );
                    autostart_daemon(options)?;
                    autostarted = true;
                } else if attempt > 0 {
                    eprintln!(
                        "proxy: connect attempt {} to {} failed: {error}",
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
    let mut command = Command::new(&options.exe);
    command.arg("--daemon");
    for arg in &options.daemon_args {
        command.arg(arg);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to autostart daemon: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_line_identifies_agent() {
        let hello = json!({"bridge": "hello", "agent": "hermes"});
        assert_eq!(hello["agent"], "hermes");
        let hello = json!({"bridge": "hello", "agent": "codex"});
        assert_eq!(hello["agent"], "codex");
    }
}
