//! Daemon mode: TCP loopback server that multiplexes multiple MCP sessions
//! (Codex and Hermes) over a single hardware-owning bridge process.
//!
//! See `run_daemon` for the entry point invoked from `main.rs` when the
//! `--daemon` flag is set.

use crate::routing::AgentId;
use crate::mcp;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Default loopback bind address for the daemon.
pub const DEFAULT_BIND: &str = "127.0.0.1:48360";

/// Internal greeting sent by a proxy as its first line to identify which
/// agent it represents. The daemon consumes it and does not forward it to
/// the MCP client.
const BRIDGE_HELLO: &str = "hello";

/// A message received from a connected session.
pub enum SessionMessage {
    /// A parsed JSON-RPC request from the session.
    Request { session_id: usize, request: Value },
    /// The session identified its agent via the hello line.
    Hello { session_id: usize, agent: AgentId },
    /// The session disconnected.
    Disconnected { session_id: usize },
    /// An I/O or parse error on the session.
    Error { session_id: usize, error: String },
}

pub struct DaemonOptions {
    pub bind: String,
    pub bridge_options: crate::Options,
}

/// Runs the daemon: owns the hardware via a single `BridgeRuntime` and
/// serves multiple MCP sessions over TCP loopback.
pub fn run_daemon(options: DaemonOptions) -> Result<(), String> {
    let listener = TcpListener::bind(&options.bind)
        .map_err(|error| format!("failed to bind {}: {error}", options.bind))?;
    let local = listener
        .local_addr()
        .map_err(|error| format!("could not resolve local addr: {error}"))?;
    eprintln!("bridge daemon listening on {local}");

    let mut bridge = crate::open_runtime(&options.bridge_options)?;
    eprintln!("{}", crate::bridge_status(&bridge, "daemon"));

    let (session_tx, session_rx) = mpsc::channel::<SessionMessage>();
    let mut sessions: Vec<SessionHandle> = Vec::new();
    let _next_session_id: usize = 1;

    // Pending long-polls: one per agent. Each entry holds the session id and
    // the deadline until which the daemon should wait before responding with
    // an empty result.
    let mut pending_polls: Vec<PendingPoll> = Vec::new();

    // Acceptor thread.
    let accept_tx = session_tx.clone();
    thread::Builder::new()
        .name("daemon-accept".to_owned())
        .spawn(move || accept_connections(&listener, accept_tx))
        .map_err(|error| format!("failed to start acceptor: {error}"))?;

    loop {
        // Drain session messages.
        while let Ok(msg) = session_rx.try_recv() {
            match msg {
                SessionMessage::Hello { session_id, agent } => {
                    handle_hello(&mut sessions, session_id, agent);
                }
                SessionMessage::Request { session_id, request } => {
                    handle_request(
                        &mut sessions,
                        session_id,
                        request,
                        &mut bridge,
                        &mut pending_polls,
                    )?;
                }
                SessionMessage::Disconnected { session_id } => {
                    remove_session(&mut sessions, session_id);
                    pending_polls.retain(|p| p.session_id != session_id);
                }
                SessionMessage::Error { session_id, error } => {
                    eprintln!("session {session_id} error: {error}");
                    remove_session(&mut sessions, session_id);
                    pending_polls.retain(|p| p.session_id != session_id);
                }
            }
        }

        // Drain serial events (only when RP2040 is present).
        let mut serial_disconnected = None;
        let mut serial_taken = bridge.serial.take();
        if let Some(runtime) = serial_taken.as_mut() {
            let mut events = Vec::new();
            while let Ok(event) = runtime.receiver.try_recv() {
                events.push(event);
            }
            for event in events {
                match event {
                    crate::serial::SerialEvent::Frame(frame)
                        if frame.frame_type == crate::wire::FrameType::CodexOutputReport =>
                    {
                        match bridge.codex_decoder.feed(&frame.payload) {
                            Ok(messages) => {
                                for message in messages {
                                    let writer = runtime.writer.as_mut();
                                    match crate::process_codex_message(
                                        message,
                                        &mut bridge.controller,
                                        &mut bridge.last_thread_status,
                                        &mut bridge.last_rgb_config,
                                        &mut bridge.fused_lcd,
                                        writer,
                                        &mut bridge.sequence,
                                        false,
                                    ) {
                                        Ok(()) => {}
                                        Err(crate::ProcessError::Controller(error)) => {
                                            crate::detach_controller_for(&mut bridge, &error)
                                        }
                                        Err(crate::ProcessError::Protocol(error)) => {
                                            eprintln!("daemon Codex parameter error: {error}")
                                        }
                                        Err(crate::ProcessError::Serial(error)) => {
                                            serial_disconnected = Some(error);
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(error) => eprintln!("daemon Codex report error: {error}"),
                        }
                    }
                    crate::serial::SerialEvent::Frame(frame)
                        if frame.frame_type == crate::wire::FrameType::Status =>
                    {
                        bridge.health.observe_status_at(Instant::now())
                    }
                    crate::serial::SerialEvent::Frame(frame)
                        if frame.frame_type == crate::wire::FrameType::Log =>
                    {
                        eprintln!("RP2040: {}", String::from_utf8_lossy(&frame.payload))
                    }
                    crate::serial::SerialEvent::Frame(_) => {}
                    crate::serial::SerialEvent::ProtocolError(error) => {
                        eprintln!("bridge protocol error: {error}")
                    }
                    crate::serial::SerialEvent::Disconnected(error) => {
                        serial_disconnected = Some(error);
                        break;
                    }
                }
            }
        }
        bridge.serial = serial_taken;
        if let Some(error) = serial_disconnected {
            eprintln!("RP2040 bridge disconnected: {error}");
            crate::replace_runtime(&mut bridge, &options.bridge_options)?;
            eprintln!("{}", crate::bridge_status(&bridge, "daemon-reconnected"));
        }

        // Health check (only with serial).
        let now = Instant::now();
        if bridge.has_serial() {
            if bridge.health.timed_out_at(now) {
                eprintln!("RP2040 health check timed out; reconnecting");
                crate::replace_runtime(&mut bridge, &options.bridge_options)?;
            } else if let Err(error) = crate::send_health_ping(&mut bridge, now) {
                eprintln!("RP2040 health check failed: {error}");
                crate::replace_runtime(&mut bridge, &options.bridge_options)?;
            }
        }

        // Controller reconnect + poll.
        crate::reconnect_controller_if_due(&mut bridge);
        crate::poll_controller(&mut bridge, false)?;

        // Resolve pending long-polls that have events or have timed out.
        resolve_pending_polls(&mut pending_polls, &mut sessions, &mut bridge, now);

        // Brief sleep to avoid busy-looping.
        thread::sleep(Duration::from_millis(10));
    }
}

struct SessionHandle {
    id: usize,
    agent: Option<AgentId>,
    writer: Sender<Result<Value, String>>,
}

struct PendingPoll {
    session_id: usize,
    agent: AgentId,
    deadline: Instant,
    request_id: Value,
}

fn accept_connections(listener: &TcpListener, tx: Sender<SessionMessage>) {
    let mut next_id = 1usize;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let id = next_id;
                next_id += 1;
                let session_tx = tx.clone();
                if let Err(error) = thread::Builder::new()
                    .name(format!("daemon-session-{id}"))
                    .spawn(move || handle_session(id, stream, session_tx))
                {
                    eprintln!("failed to spawn session thread: {error}");
                }
            }
            Err(error) => eprintln!("accept error: {error}"),
        }
    }
}

fn handle_session(id: usize, stream: TcpStream, tx: Sender<SessionMessage>) {
    let peer = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "unknown".to_owned());
    eprintln!("session {id} connected from {peer}");

    let read_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(error) => {
            let _ = tx.send(SessionMessage::Error {
                session_id: id,
                error: format!("clone failed: {error}"),
            });
            return;
        }
    };

    let (_writer_tx, writer_rx) = mpsc::channel::<Result<Value, String>>();
    let mut write_stream = stream;

    // Writer thread.
    let _writer_thread = thread::Builder::new()
        .name(format!("daemon-session-{id}-write"))
        .spawn(move || {
            for message in writer_rx.iter() {
                let line = match message {
                    Ok(value) => serde_json::to_string(&value).unwrap_or_default(),
                    Err(error) => serde_json::to_string(&mcp::error_response(
                        None,
                        -32001,
                        error,
                    ))
                    .unwrap_or_default(),
                };
                if let Err(error) = write_stream
                    .write_all(line.as_bytes())
                    .and_then(|_| write_stream.write_all(b"\n"))
                    .and_then(|_| write_stream.flush())
                {
                    eprintln!("session {id} write failed: {error}");
                    break;
                }
            }
        });

    // Reader loop.
    let reader = BufReader::new(read_stream);
    let mut first_line = true;
    for line in reader.lines() {
        match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => {
                if first_line {
                    first_line = false;
                    if let Some(agent) = parse_hello(&line) {
                        let _ = tx.send(SessionMessage::Hello {
                            session_id: id,
                            agent,
                        });
                        continue;
                    }
                    // Fall through: treat as a normal MCP request.
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(request) => {
                        if tx
                            .send(SessionMessage::Request {
                                session_id: id,
                                request,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(SessionMessage::Error {
                            session_id: id,
                            error: format!("invalid JSON: {error}"),
                        });
                        return;
                    }
                }
            }
            Err(error) => {
                let _ = tx.send(SessionMessage::Error {
                    session_id: id,
                    error: format!("read failed: {error}"),
                });
                return;
            }
        }
    }
    let _ = tx.send(SessionMessage::Disconnected { session_id: id });
}

fn parse_hello(line: &str) -> Option<AgentId> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("bridge").and_then(Value::as_str) == Some(BRIDGE_HELLO) {
        value
            .get("agent")
            .and_then(Value::as_str)
            .and_then(|s| AgentId::parse(s).ok())
    } else {
        None
    }
}

fn handle_hello(sessions: &mut Vec<SessionHandle>, session_id: usize, agent: AgentId) {
    // Replace any existing session for the same agent (the new one wins).
    sessions.retain(|s| s.agent != Some(agent));
    // Find the session by id and set its agent.
    if let Some(session) = sessions.iter_mut().find(|s| s.id == session_id) {
        session.agent = Some(agent);
        eprintln!("session {session_id} registered as agent {}", agent.as_str());
    }
}

fn remove_session(sessions: &mut Vec<SessionHandle>, session_id: usize) {
    sessions.retain(|s| s.id != session_id);
}

fn find_session_agent(sessions: &[SessionHandle], session_id: usize) -> Option<AgentId> {
    sessions
        .iter()
        .find(|s| s.id == session_id)
        .and_then(|s| s.agent)
}

fn send_to_session(sessions: &[SessionHandle], session_id: usize, message: Value) {
    if let Some(session) = sessions.iter().find(|s| s.id == session_id) {
        let _ = session.writer.send(Ok(message));
    }
}

fn handle_request(
    sessions: &mut Vec<SessionHandle>,
    session_id: usize,
    request: Value,
    bridge: &mut crate::BridgeRuntime,
    pending_polls: &mut Vec<PendingPoll>,
) -> Result<(), String> {
    let id = request.get("id");
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        if id.is_some() {
            send_to_session(
                sessions,
                session_id,
                mcp::error_response(id, -32600, "MCP request requires method"),
            );
        }
        return Ok(());
    };
    if method.starts_with("notifications/") {
        return Ok(());
    }
    let Some(id) = id else {
        return Ok(());
    };
    let agent = find_session_agent(sessions, session_id);
    let result = match method {
        "initialize" => json!({
            "protocolVersion": mcp::PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "micro-emu-bridge", "version": env!("CARGO_PKG_VERSION")},
            "instructions": match agent {
                Some(AgentId::Hermes) => "Hermes agent. Use bridge_status first, then poll_events to receive physical key presses (AG03-AG05). set_thread_status writes LCD slots 4-6.",
                _ => "Codex agent. Use bridge_status first. Hardware actions target the RP2040 on the configured serial port.",
            }
        }),
        "ping" => json!({}),
        "tools/list" => mcp::tools_for(agent),
        "tools/call" => call_tool_for(&request, bridge, agent, pending_polls, session_id, id.clone()),
        _ => {
            send_to_session(
                sessions,
                session_id,
                mcp::error_response(Some(id), -32601, format!("unknown MCP method: {method}")),
            );
            return Ok(());
        }
    };
    send_to_session(sessions, session_id, mcp::response(id, result));
    Ok(())
}

fn call_tool_for(
    request: &Value,
    bridge: &mut crate::BridgeRuntime,
    agent: Option<AgentId>,
    pending_polls: &mut Vec<PendingPoll>,
    session_id: usize,
    id: Value,
) -> Value {
    let params = request.get("params").unwrap_or(&Value::Null);
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return mcp::tool_error("tools/call requires params.name");
    };
    let arguments = request
        .get("params")
        .and_then(|p| p.get("arguments"))
        .unwrap_or(&Value::Null);

    // Check tool availability for the agent.
    if !mcp::tool_available(name, agent) {
        return mcp::tool_error(format!(
            "tool {name} is not available for agent {}",
            agent.map(|a| a.as_str()).unwrap_or("unknown")
        ));
    }

    match name {
        "bridge_status" => mcp::text_result(crate::bridge_status(bridge, "daemon")),
        "emit_key" => crate::call_emit_key(bridge, arguments),
        "send_codex_message" => crate::call_send_codex_message(bridge, arguments),
        "set_thread_status" => call_set_thread_status(bridge, arguments, agent),
        "set_display_context" => crate::call_set_display_context(bridge, arguments),
        "set_rgb_config" => crate::call_set_rgb_config(bridge, arguments),
        "device_status" => crate::call_device_status(bridge),
        "poll_events" => {
            let timeout_ms = arguments
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(25_000);
            let agent = agent.unwrap_or(AgentId::Codex);
            let queue = bridge.routing.queue_mut(agent);
            if !queue.is_empty() {
                let events: Vec<Value> = queue
                    .drain()
                    .into_iter()
                    .map(|e| {
                        json!({"key": e.key, "pressed": e.pressed, "ts": e.timestamp_ms})
                    })
                    .collect();
                mcp::text_result(json!({"events": events}))
            } else if timeout_ms > 0 {
                // Defer the response: register a pending poll.
                // Cancel any previous pending poll for this session.
                pending_polls.retain(|p| p.session_id != session_id);
                pending_polls.push(PendingPoll {
                    session_id,
                    agent,
                    deadline: Instant::now() + Duration::from_millis(timeout_ms),
                    request_id: id,
                });
                // Return a sentinel that the caller should not send.
                Value::Null
            } else {
                mcp::text_result(json!({"events": []}))
            }
        }
        _ => mcp::tool_error(format!("unknown MCP tool: {name}")),
    }
}

fn call_set_thread_status(
    bridge: &mut crate::BridgeRuntime,
    arguments: &Value,
    agent: Option<AgentId>,
) -> Value {
    let Some(status) = arguments.get("status") else {
        return mcp::tool_error("set_thread_status requires status");
    };
    if !status.is_array() {
        return mcp::tool_error("status must be an array");
    }
    let agent = agent.unwrap_or(AgentId::Codex);
    let fused = match bridge.fused_lcd.merge_from_agent(agent, status) {
        Ok(fused) => fused,
        Err(error) => return mcp::tool_error(error),
    };
    // Apply the fused array to the physical controller.
    if let Some(device) = bridge.controller.as_mut() {
        let fused_value = Value::Array(fused);
        if let Err(error) = device.apply_thread_status(&fused_value) {
            crate::detach_controller_for(bridge, &error);
            return mcp::tool_error(format!("controller apply failed: {error}"));
        }
    }
    // If we have a serial connection, also forward the thstatus to ChatGPT
    // so the HID side stays in sync. Only Codex's set_thread_status is
    // forwarded (Hermes slots are not part of the Codex Micro protocol).
    if agent == AgentId::Codex && bridge.has_serial() {
        let message = json!({"m": "v.oai.thstatus", "p": status});
        let _ = bridge.send_codex(&message);
    }
    mcp::text_result(json!({"updated": true, "agent": agent.as_str()}))
}

fn resolve_pending_polls(
    pending_polls: &mut Vec<PendingPoll>,
    sessions: &mut Vec<SessionHandle>,
    bridge: &mut crate::BridgeRuntime,
    now: Instant,
) {
    let mut resolved = Vec::new();
    pending_polls.retain(|poll| {
        let queue = bridge.routing.queue(poll.agent);
        if !queue.is_empty() || now >= poll.deadline {
            let events: Vec<Value> = bridge
                .routing
                .queue_mut(poll.agent)
                .drain()
                .into_iter()
                .map(|e| json!({"key": e.key, "pressed": e.pressed, "ts": e.timestamp_ms}))
                .collect();
            let result = mcp::text_result(json!({"events": events}));
            send_to_session(sessions, poll.session_id, mcp::response(&poll.request_id, result));
            resolved.push(poll.session_id);
            false
        } else {
            true
        }
    });
    let _ = resolved;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hello_line() {
        let line = r#"{"bridge":"hello","agent":"hermes"}"#;
        assert_eq!(parse_hello(line), Some(AgentId::Hermes));
        let line = r#"{"bridge":"hello","agent":"codex"}"#;
        assert_eq!(parse_hello(line), Some(AgentId::Codex));
        let line = r#"{"jsonrpc":"2.0","method":"initialize","id":1}"#;
        assert_eq!(parse_hello(line), None);
    }

    #[test]
    fn hello_replaces_existing_session_for_same_agent() {
        let mut sessions = vec![SessionHandle {
            id: 1,
            agent: Some(AgentId::Hermes),
            writer: mpsc::channel().0,
        }];
        // New session id=2 claims Hermes.
        sessions.push(SessionHandle {
            id: 2,
            agent: None,
            writer: mpsc::channel().0,
        });
        handle_hello(&mut sessions, 2, AgentId::Hermes);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, 2);
        assert_eq!(sessions[0].agent, Some(AgentId::Hermes));
    }
}
