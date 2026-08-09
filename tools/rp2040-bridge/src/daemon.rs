//! Daemon mode: TCP loopback server that multiplexes multiple MCP sessions
//! (Codex and Hermes) over a single hardware-owning bridge process.
//!
//! See `run_daemon` for the entry point invoked from `main.rs` when the
//! `--daemon` flag is set.

use crate::controller::PhysicalController;
use crate::mcp;
use crate::routing::{ActiveSet, AgentId, Partition};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn log_context() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    format!("ts={millis} pid={}", std::process::id())
}

/// Default loopback bind address for the daemon.
pub const DEFAULT_BIND: &str = "127.0.0.1:48360";

/// Debounce period before a repartition is applied after the active set
/// changes. Agent restarts and workspace switches cause brief membership
/// fluctuations; this prevents LCD churn.
const REPARTITION_DEBOUNCE: Duration = Duration::from_millis(750);

/// Internal greeting sent by a proxy as its first line to identify which
/// agent it represents. The daemon consumes it and does not forward it to
/// the MCP client.
const BRIDGE_HELLO: &str = "hello";

/// A message received from a connected session.
#[derive(Clone, Debug)]
pub(crate) struct HelloInfo {
    agent: AgentId,
    instance_id: Option<String>,
    focus_capable: bool,
}

pub enum SessionMessage {
    /// Registers the response writer before the session sends its hello.
    Connected {
        session_id: usize,
        writer: Sender<Result<Value, String>>,
    },
    /// A parsed JSON-RPC request from the session.
    Request { session_id: usize, request: Value },
    /// The session identified its agent via the hello line.
    Hello { session_id: usize, info: HelloInfo },
    /// A Stream Deck plugin controller session announced itself. The daemon
    /// creates a `PluginController` from the provided channels and registers
    /// it as an aux controller.
    ControllerHello {
        session_id: usize,
        instance_id: String,
        task_slots: usize,
        events: crate::plugin_controller::PluginEventReceiver,
        writer: crate::plugin_controller::PluginWriter,
    },
    /// The session disconnected.
    Disconnected { session_id: usize },
    /// An I/O or parse error on the session.
    Error { session_id: usize, error: String },
}

pub struct DaemonOptions {
    pub bind: String,
    pub bridge_options: crate::Options,
}

fn retry_serial_connection(
    bridge: &mut crate::BridgeRuntime,
    options: &crate::Options,
    retry_at: &mut Option<Instant>,
    retry_delay: &mut Duration,
    reason: &str,
) {
    match crate::replace_runtime(bridge, options) {
        Ok(()) => {
            eprintln!("{}", crate::bridge_status(bridge, "daemon-reconnected"));
            *retry_at = None;
            *retry_delay = crate::SERIAL_RETRY_INITIAL_DELAY;
        }
        Err(error) => {
            eprintln!("RP2040 reconnect pending ({reason}): {error}");
            *retry_at = Some(Instant::now() + *retry_delay);
            *retry_delay = (*retry_delay * 2).min(crate::SERIAL_RETRY_MAX_DELAY);
        }
    }
}

/// Computes the effective active set: agents with live MCP sessions, plus
/// Codex if the RP2040 serial link is up (ChatGPT drives Codex over HID
/// without an MCP session).
fn effective_active_set(session_agents: ActiveSet, codex_hardware_active: bool) -> ActiveSet {
    let mut set = session_agents;
    if codex_hardware_active {
        set.insert(AgentId::Codex);
    }
    set
}

/// Runs the daemon: owns the hardware via a single `BridgeRuntime` and
/// serves multiple MCP sessions over TCP loopback.
pub fn run_daemon(options: DaemonOptions) -> Result<(), String> {
    eprintln!(
        "daemon [{}] starting bind={} port={} controller={} serial={:?}",
        log_context(),
        options.bind,
        options.bridge_options.port,
        options.bridge_options.controller.as_str(),
        options.bridge_options.controller_serial
    );
    let listener = TcpListener::bind(&options.bind)
        .map_err(|error| format!("failed to bind {}: {error}", options.bind))?;
    let local = listener
        .local_addr()
        .map_err(|error| format!("could not resolve local addr: {error}"))?;
    eprintln!("daemon [{}] listening on {local}", log_context());

    let mut bridge = crate::open_runtime(&options.bridge_options)?;
    bridge.task_mode = true;
    // Initialize the partition: Codex is active if the RP2040 is present.
    let mut session_agents = ActiveSet::new();
    let mut codex_hardware_active = bridge.has_serial();
    let mut pending_repartition_at: Option<Instant> = None;
    {
        let active = effective_active_set(session_agents, codex_hardware_active);
        bridge.partition = Partition::compute(active);
    }
    eprintln!("{}", crate::bridge_status(&bridge, "daemon"));

    let (session_tx, session_rx) = mpsc::channel::<SessionMessage>();
    let mut sessions: Vec<SessionHandle> = Vec::new();
    let _next_session_id: usize = 1;

    // Pending long-polls: one per agent. Each entry holds the session id and
    // the deadline until which the daemon should wait before responding with
    // an empty result.
    let mut pending_polls: Vec<PendingPoll> = Vec::new();
    let mut serial_retry_at = (!bridge.has_serial() && options.bridge_options.port != "none")
        .then(Instant::now);
    let mut serial_retry_delay = crate::SERIAL_RETRY_INITIAL_DELAY;

    // Plugin controller sessions: maps session_id to the device_id of the
    // PluginController registered in aux_controllers.
    let mut plugin_sessions: Vec<(usize, String)> = Vec::new();

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
                SessionMessage::Connected { session_id, writer } => {
                    eprintln!(
                        "daemon [{}] session connected id={session_id}",
                        log_context()
                    );
                    sessions.push(SessionHandle {
                        id: session_id,
                        agent: None,
                        instance_id: None,
                        focus_capable: false,
                        writer,
                        events: VecDeque::new(),
                    });
                }
                SessionMessage::Hello { session_id, info } => {
                    eprintln!(
                        "daemon [{}] hello session={} agent={} instance={:?} focus={}",
                        log_context(),
                        session_id,
                        info.agent.as_str(),
                        info.instance_id,
                        info.focus_capable
                    );
                    let agent = info.agent;
                    handle_hello(&mut sessions, session_id, info);
                    session_agents.insert(agent);
                    pending_repartition_at = Some(Instant::now() + REPARTITION_DEBOUNCE);
                }
                SessionMessage::Request {
                    session_id,
                    request,
                } => {
                    let method = request
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or("<notification-or-invalid>");
                    eprintln!(
                        "daemon [{}] request session={} method={} id={}",
                        log_context(),
                        session_id,
                        method,
                        request
                            .get("id")
                            .map(Value::to_string)
                            .unwrap_or_else(|| "-".to_owned())
                    );
                    handle_request(
                        &mut sessions,
                        session_id,
                        request,
                        &mut bridge,
                        &mut pending_polls,
                    )?;
                }
                SessionMessage::ControllerHello {
                    session_id,
                    instance_id,
                    task_slots,
                    events,
                    writer,
                } => {
                    eprintln!(
                        "daemon [{}] controller hello session={} instance={instance_id} slots={task_slots}",
                        log_context(),
                        session_id
                    );
                    let controller = crate::plugin_controller::PluginController::new(
                        instance_id,
                        task_slots,
                        events,
                        writer,
                    );
                    let device_id = controller.device_id();
                    let slots = controller.task_slot_count();
                    if slots > 0 {
                        bridge.task_board.set_device(device_id.clone(), slots, true);
                    }
                    bridge
                        .aux_controllers
                        .push((device_id.clone(), Box::new(controller), slots));
                    plugin_sessions.push((session_id, device_id.clone()));
                    let _ = crate::refresh_task_board(&mut bridge);
                }
                SessionMessage::Disconnected { session_id } => {
                    eprintln!(
                        "daemon [{}] session disconnected id={session_id}",
                        log_context()
                    );
                    // Plugin controller session: detach the aux controller.
                    if let Some(device_id) = remove_plugin_session(&mut plugin_sessions, session_id)
                    {
                        detach_plugin_controller(&mut bridge, &device_id);
                        let _ = crate::refresh_task_board(&mut bridge);
                        continue;
                    }
                    let removed_agent = find_session_agent(&sessions, session_id);
                    bridge.task_board.disconnect_session(session_id, now_ms());
                    let _ = crate::refresh_task_board(&mut bridge);
                    remove_session(&mut sessions, session_id);
                    pending_polls.retain(|p| p.session_id != session_id);
                    if let Some(agent) = removed_agent {
                        if !sessions.iter().any(|s| s.agent == Some(agent)) {
                            session_agents.remove(agent);
                            pending_repartition_at = Some(Instant::now() + REPARTITION_DEBOUNCE);
                        }
                    }
                }
                SessionMessage::Error { session_id, error } => {
                    eprintln!(
                        "daemon [{}] session error id={session_id}: {error}",
                        log_context()
                    );
                    if let Some(device_id) = remove_plugin_session(&mut plugin_sessions, session_id)
                    {
                        detach_plugin_controller(&mut bridge, &device_id);
                        let _ = crate::refresh_task_board(&mut bridge);
                        continue;
                    }
                    let removed_agent = find_session_agent(&sessions, session_id);
                    bridge.task_board.disconnect_session(session_id, now_ms());
                    let _ = crate::refresh_task_board(&mut bridge);
                    remove_session(&mut sessions, session_id);
                    pending_polls.retain(|p| p.session_id != session_id);
                    if let Some(agent) = removed_agent {
                        if !sessions.iter().any(|s| s.agent == Some(agent)) {
                            session_agents.remove(agent);
                            pending_repartition_at = Some(Instant::now() + REPARTITION_DEBOUNCE);
                        }
                    }
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
                                    if bridge.task_mode
                                        && crate::codex::method(&message) == Some("v.oai.thstatus")
                                    {
                                        if let Some(parameters) =
                                            message.get("p").or_else(|| message.get("params"))
                                        {
                                            // The serial status is an explicit Codex update even
                                            // when an optional task field cannot be adapted. Keep
                                            // it authoritative and let the renderer fall back to
                                            // the fused status buffer rather than standby cards.
                                            bridge.has_explicit_task_state = true;
                                            match bridge
                                                .task_board
                                                .publish_codex_hid_status(parameters, now_ms())
                                            {
                                                Ok(()) => eprintln!(
                                                    "Codex HID status accepted entries={}",
                                                    parameters
                                                        .as_array()
                                                        .map(Vec::len)
                                                        .unwrap_or(0)
                                                ),
                                                Err(error) => eprintln!(
                                                    "Codex task status adapter fallback: {error}"
                                                ),
                                            }
                                            let _ = crate::refresh_task_board(&mut bridge);
                                        }
                                    }
                                    let writer = runtime.writer.as_mut();
                                    match crate::process_codex_message(
                                        message,
                                        &mut bridge.controller,
                                        &mut bridge.last_thread_status,
                                        &mut bridge.last_rgb_config,
                                        &mut bridge.fused_lcd,
                                        &bridge.partition,
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
            retry_serial_connection(
                &mut bridge,
                &options.bridge_options,
                &mut serial_retry_at,
                &mut serial_retry_delay,
                "device event",
            );
        }

        // Retry a port that was still absent or busy after resume. Keep the
        // daemon alive while Windows finishes re-enumerating the USB device.
        let now = Instant::now();
        if !bridge.has_serial()
            && options.bridge_options.port != "none"
            && serial_retry_at.is_some_and(|retry_at| now >= retry_at)
        {
            retry_serial_connection(
                &mut bridge,
                &options.bridge_options,
                &mut serial_retry_at,
                &mut serial_retry_delay,
                "port unavailable",
            );
        }

        // Health check (only with serial).
        if bridge.has_serial() {
            if bridge.health.timed_out_at(now) {
                eprintln!("RP2040 health check timed out; reconnecting");
                retry_serial_connection(
                    &mut bridge,
                    &options.bridge_options,
                    &mut serial_retry_at,
                    &mut serial_retry_delay,
                    "health timeout",
                );
            } else if let Err(error) = crate::send_health_ping(&mut bridge, now) {
                eprintln!("RP2040 health check failed: {error}");
                retry_serial_connection(
                    &mut bridge,
                    &options.bridge_options,
                    &mut serial_retry_at,
                    &mut serial_retry_delay,
                    "health write",
                );
            }
        }

        // Controller reconnect + poll.
        crate::reconnect_controller_if_due(&mut bridge);
        bridge.task_board.expire(now_ms());
        let _ = crate::refresh_task_board(&mut bridge);
        crate::poll_controller(&mut bridge, false)?;
        for (session_id, event) in bridge.pending_task_events.drain(..) {
            if let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) {
                session.events.push_back(event);
            }
        }

        // Track RP2040 attach/detach for Codex hardware activity.
        let now_has_serial = bridge.has_serial();
        if now_has_serial != codex_hardware_active {
            codex_hardware_active = now_has_serial;
            pending_repartition_at = Some(now + REPARTITION_DEBOUNCE);
        }

        // Apply a debounced repartition if the active set has changed.
        if let Some(repartition_at) = pending_repartition_at {
            if now >= repartition_at {
                pending_repartition_at = None;
                let active = effective_active_set(session_agents, codex_hardware_active);
                let new_partition = Partition::compute(active);
                eprintln!(
                    "repartition: active={:?} owners={:?}",
                    active.iter(),
                    new_partition.owners_json()
                );
                bridge.partition = new_partition.clone();
                // Re-render through the centralized desired-state path.  An
                // empty fused buffer is not an explicit Codex update; sending
                // it directly would clear the connection-default cards while
                // the MCP session is merely establishing its partition.
                let _ = crate::refresh_task_board(&mut bridge);
                // Notify all active agents about the new partition.
                let active_list = active.iter();
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                for agent in &active_list {
                    bridge.routing.push_partition_event(
                        *agent,
                        &new_partition,
                        &active_list,
                        now_ms,
                    );
                }
            }
        }

        // Resolve pending long-polls that have events or have timed out.
        resolve_pending_polls(&mut pending_polls, &mut sessions, &mut bridge, now);

        // Brief sleep to avoid busy-looping.
        thread::sleep(Duration::from_millis(10));
    }
}

struct SessionHandle {
    id: usize,
    agent: Option<AgentId>,
    instance_id: Option<String>,
    focus_capable: bool,
    writer: Sender<Result<Value, String>>,
    events: VecDeque<Value>,
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
    eprintln!(
        "daemon [{}] session {id} connected from {peer}",
        log_context()
    );

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

    let mut write_stream = stream;
    let mut reader = BufReader::new(read_stream);

    // Read the first line to determine the session type (controller hello,
    // agent hello, or a raw MCP request) before setting up the writer.
    let mut first_line = String::new();
    let first_read = reader.read_line(&mut first_line);
    let first_line_trimmed = first_line.trim();
    let first_line_owned = if first_line_trimmed.is_empty() {
        String::new()
    } else {
        first_line_trimmed.to_owned()
    };

    if let Err(error) = first_read {
        let _ = tx.send(SessionMessage::Error {
            session_id: id,
            error: format!("read failed: {error}"),
        });
        return;
    }

    // Stream Deck plugin controller session.
    if let Some((instance_id, task_slots)) =
        crate::plugin_controller::parse_controller_hello(&first_line_owned)
    {
        handle_plugin_session(id, write_stream, reader, tx, instance_id, task_slots);
        return;
    }

    // MCP agent session: set up the response writer and register the session.
    let (writer_tx, writer_rx) = mpsc::channel::<Result<Value, String>>();
    let _writer_thread = thread::Builder::new()
        .name(format!("daemon-session-{id}-write"))
        .spawn(move || {
            for message in writer_rx.iter() {
                let line = match message {
                    Ok(value) => serde_json::to_string(&value).unwrap_or_default(),
                    Err(error) => serde_json::to_string(&mcp::error_response(None, -32001, error))
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

    if tx
        .send(SessionMessage::Connected {
            session_id: id,
            writer: writer_tx,
        })
        .is_err()
    {
        return;
    }

    // Process the first line (agent hello or MCP request), then the rest.
    if !first_line_owned.is_empty() {
        if let Some(mut info) = parse_hello_info(&first_line_owned) {
            if info.instance_id.is_none() {
                info.instance_id = Some(format!("session-{id}"));
            }
            let _ = tx.send(SessionMessage::Hello {
                session_id: id,
                info,
            });
        } else {
            forward_mcp_line(&tx, id, &first_line_owned);
        }
    }

    for line in reader.lines() {
        match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => forward_mcp_line(&tx, id, &line),
            Err(error) => {
                let _ = tx.send(SessionMessage::Error {
                    session_id: id,
                    error: format!("read failed: {error}"),
                });
                return;
            }
        }
    }
    eprintln!("daemon [{}] session {id} reader reached EOF", log_context());
    let _ = tx.send(SessionMessage::Disconnected { session_id: id });
}

fn forward_mcp_line(tx: &Sender<SessionMessage>, id: usize, line: &str) {
    match serde_json::from_str::<Value>(line) {
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
        }
    }
}

/// Handles a Stream Deck plugin controller session: forwards inbound lines to
/// the controller's event channel and writes outbound render lines to the
/// socket.
fn handle_plugin_session(
    id: usize,
    write_stream: TcpStream,
    reader: BufReader<TcpStream>,
    tx: Sender<SessionMessage>,
    instance_id: String,
    task_slots: usize,
) {
    eprintln!(
        "daemon [{}] plugin controller session {id} instance={instance_id} slots={task_slots}",
        log_context()
    );

    let (events_tx, events_rx) = mpsc::channel::<Value>();
    let (plugin_writer_tx, plugin_writer_rx) = mpsc::channel::<Value>();

    // Plugin writer thread: drains outbound render lines and writes them to
    // the socket as newline-delimited JSON.
    let write_thread = thread::Builder::new()
        .name(format!("daemon-plugin-{id}-write"))
        .spawn(move || {
            let mut write_stream = write_stream;
            for message in plugin_writer_rx.iter() {
                let line = serde_json::to_string(&message).unwrap_or_default();
                if let Err(error) = write_stream
                    .write_all(line.as_bytes())
                    .and_then(|_| write_stream.write_all(b"\n"))
                    .and_then(|_| write_stream.flush())
                {
                    eprintln!("plugin session {id} write failed: {error}");
                    break;
                }
            }
        });

    if let Err(error) = write_thread {
        let _ = tx.send(SessionMessage::Error {
            session_id: id,
            error: format!("failed to spawn plugin writer: {error}"),
        });
        return;
    }

    if tx
        .send(SessionMessage::ControllerHello {
            session_id: id,
            instance_id,
            task_slots,
            events: events_rx,
            writer: plugin_writer_tx,
        })
        .is_err()
    {
        return;
    }

    // Reader loop: forward all inbound lines to the controller's event channel.
    for line in reader.lines() {
        match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => {
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        if events_tx.send(value).is_err() {
                            // Controller was dropped (daemon detach); stop reading.
                            return;
                        }
                    }
                    Err(error) => {
                        eprintln!("plugin session {id} invalid JSON: {error}");
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
    eprintln!(
        "daemon [{}] plugin session {id} reader reached EOF",
        log_context()
    );
    let _ = tx.send(SessionMessage::Disconnected { session_id: id });
}

fn parse_hello_info(line: &str) -> Option<HelloInfo> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("bridge").and_then(Value::as_str) != Some(BRIDGE_HELLO) {
        return None;
    }
    let agent = value
        .get("agent")
        .and_then(Value::as_str)
        .and_then(|s| AgentId::parse(s).ok())?;
    let instance_id = value
        .get("instance_id")
        .or_else(|| value.get("instance"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.chars().count() <= 160)
        .map(str::to_owned);
    let focus_capable = value
        .get("focus")
        .and_then(Value::as_bool)
        .or_else(|| {
            value
                .get("capabilities")
                .and_then(|v| v.get("focus"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);
    Some(HelloInfo {
        agent,
        instance_id,
        focus_capable,
    })
}

fn handle_hello(sessions: &mut Vec<SessionHandle>, session_id: usize, info: HelloInfo) {
    if let Some(session) = sessions.iter_mut().find(|s| s.id == session_id) {
        session.agent = Some(info.agent);
        session.instance_id = Some(
            info.instance_id
                .unwrap_or_else(|| format!("session-{session_id}")),
        );
        session.focus_capable = info.focus_capable;
        eprintln!(
            "session {session_id} registered as agent {} (instance {}, focus={})",
            info.agent.as_str(),
            session.instance_id.as_deref().unwrap_or("unknown"),
            session.focus_capable
        );
    }
}

fn remove_session(sessions: &mut Vec<SessionHandle>, session_id: usize) {
    sessions.retain(|s| s.id != session_id);
}

/// Removes a plugin session mapping and returns the device_id if found.
fn remove_plugin_session(
    plugin_sessions: &mut Vec<(usize, String)>,
    session_id: usize,
) -> Option<String> {
    let pos = plugin_sessions
        .iter()
        .position(|(id, _)| *id == session_id)?;
    let (_, device_id) = plugin_sessions.remove(pos);
    Some(device_id)
}

/// Detaches a plugin controller from `aux_controllers` by device_id, shutting
/// it down and removing it from the task board.
fn detach_plugin_controller(bridge: &mut crate::BridgeRuntime, device_id: &str) {
    let mut survivors = Vec::new();
    let mut removed = false;
    for (id, mut device, slots) in std::mem::take(&mut bridge.aux_controllers) {
        if id == device_id {
            device.shutdown();
            eprintln!("plugin controller {device_id} detached");
            removed = true;
        } else {
            survivors.push((id, device, slots));
        }
    }
    bridge.aux_controllers = survivors;
    if removed {
        bridge.task_board.set_device(device_id, 0, false);
    }
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
    let Some(agent) = agent else {
        send_to_session(
            sessions,
            session_id,
            mcp::error_response(
                Some(id),
                -32000,
                "daemon session must send a valid bridge hello first",
            ),
        );
        return Ok(());
    };
    let instructions = match Some(agent) {
        Some(a) => {
            let keys = bridge.partition.keys_for(a);
            let slots = bridge.partition.slots_for(a);
            format!(
                "{} agent. Use bridge_status first. Your keys: {:?}, LCD slots: {:?}. \
                 Use poll_events to receive key presses and partition change notifications. \
                 The colored numbered LCD cards and READY dashboard shown before an explicit update are standby indicators only. \
                 Publish live task state with publish_tasks or set_thread_status. {}",
                a.as_str(),
                keys,
                slots,
                mcp::DISPLAY_CONTEXT_INSTRUCTIONS
            )
        }
        None => "Use bridge_status first. Hardware actions target the RP2040 on the configured serial port.".to_owned(),
    };
    let result = match method {
        "initialize" => json!({
            "protocolVersion": mcp::PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "micro-emu-bridge", "version": env!("CARGO_PKG_VERSION")},
            "instructions": instructions
        }),
        "ping" => json!({}),
        "resources/list" => json!({"resources": []}),
        "resources/templates/list" => json!({"resourceTemplates": []}),
        "tools/list" => mcp::daemon_tools_for(Some(agent)),
        "tools/call" => call_tool_for(
            &request,
            sessions,
            bridge,
            Some(agent),
            pending_polls,
            session_id,
            id.clone(),
        ),
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
    sessions: &mut Vec<SessionHandle>,
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
        "bridge_status" => mcp::text_result(daemon_bridge_status(bridge, sessions)),
        "emit_key" => crate::call_emit_key(bridge, arguments),
        "send_codex_message" => crate::call_send_codex_message(bridge, arguments),
        "set_thread_status" => call_set_thread_status(bridge, arguments, agent, session_id),
        "publish_tasks" => {
            let agent_id = agent.unwrap_or(AgentId::Codex);
            match bridge
                .task_board
                .publish_tasks(session_id, agent_id, arguments, now_ms())
            {
                Ok(result) => {
                    bridge.has_explicit_task_state = true;
                    let _ = crate::refresh_task_board(bridge);
                    push_session_event(
                        sessions,
                        session_id,
                        json!({"type":"layout_changed","tasks":result.get("tasks"),"ts":now_ms()}),
                    );
                    mcp::text_result(result)
                }
                Err(error) => mcp::tool_error(error),
            }
        }
        "set_display_context" => crate::call_set_display_context(bridge, arguments),
        "set_rgb_config" if bridge.task_mode => {
            mcp::tool_error("set_rgb_config is daemon-managed; configure RGB on the bridge")
        }
        "set_rgb_config" => crate::call_set_rgb_config(bridge, arguments),
        "device_status" => crate::call_device_status(bridge),
        "poll_events" => {
            let timeout_ms = arguments
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(25_000);
            let agent = agent.unwrap_or(AgentId::Codex);
            let session_events = drain_session_events(sessions, session_id);
            if !session_events.is_empty() {
                mcp::text_result(json!({"events": session_events}))
            } else {
                let queue = bridge.routing.queue_mut(agent);
                if !queue.is_empty() {
                    let events: Vec<Value> =
                        queue.drain().into_iter().map(|e| e.to_json()).collect();
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
        }
        _ => mcp::tool_error(format!("unknown MCP tool: {name}")),
    }
}

fn call_set_thread_status(
    bridge: &mut crate::BridgeRuntime,
    arguments: &Value,
    agent: Option<AgentId>,
    session_id: usize,
) -> Value {
    let Some(status) = arguments.get("status") else {
        return mcp::tool_error("set_thread_status requires status");
    };
    if !status.is_array() {
        return mcp::tool_error("status must be an array");
    }
    let agent = agent.unwrap_or(AgentId::Codex);
    if bridge.task_mode {
        return match bridge
            .task_board
            .publish_legacy_status(session_id, agent, status, now_ms())
        {
            Ok(result) => {
                bridge.has_explicit_task_state = true;
                let _ = crate::refresh_task_board(bridge);
                mcp::text_result(result)
            }
            Err(error) => mcp::tool_error(error),
        };
    }
    let fused = match bridge
        .fused_lcd
        .merge_from_agent(agent, status, &bridge.partition)
    {
        Ok(fused) => {
            bridge.has_explicit_task_state = true;
            fused
        }
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

fn daemon_bridge_status(bridge: &crate::BridgeRuntime, sessions: &[SessionHandle]) -> Value {
    let mut status = crate::bridge_status(bridge, "daemon");
    let session_values = sessions
        .iter()
        .map(|session| {
            json!({
                "session_id": session.id,
                "agent": session.agent.map(|agent| agent.as_str()),
                "instance_id": session.instance_id,
                "focus_capable": session.focus_capable,
                "queue_depth": session.events.len()
            })
        })
        .collect::<Vec<_>>();
    if let Some(object) = status.as_object_mut() {
        object.insert("sessions".to_owned(), json!(session_values));
        object.insert("sessionCount".to_owned(), json!(session_values.len()));
    }
    status
}
fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn push_session_event(sessions: &mut [SessionHandle], session_id: usize, event: Value) {
    if let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) {
        if session.events.len() >= 256 {
            session.events.pop_front();
        }
        session.events.push_back(event);
    }
}

fn drain_session_events(sessions: &mut Vec<SessionHandle>, session_id: usize) -> Vec<Value> {
    sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .map(|session| session.events.drain(..).collect())
        .unwrap_or_default()
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
        let session_has_events = sessions
            .iter()
            .find(|session| session.id == poll.session_id)
            .is_some_and(|session| !session.events.is_empty());
        if session_has_events || !queue.is_empty() || now >= poll.deadline {
            let mut events = drain_session_events(sessions, poll.session_id);
            if events.is_empty() {
                events = bridge
                    .routing
                    .queue_mut(poll.agent)
                    .drain()
                    .into_iter()
                    .map(|e| e.to_json())
                    .collect();
            }
            let result = mcp::text_result(json!({"events": events}));
            send_to_session(
                sessions,
                poll.session_id,
                mcp::response(&poll.request_id, result),
            );
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
        assert_eq!(
            parse_hello_info(line).map(|i| i.agent),
            Some(AgentId::Hermes)
        );
        let line = r#"{"bridge":"hello","agent":"codex"}"#;
        assert_eq!(
            parse_hello_info(line).map(|i| i.agent),
            Some(AgentId::Codex)
        );
        let line = r#"{"bridge":"hello","agent":"zcode"}"#;
        assert_eq!(
            parse_hello_info(line).map(|i| i.agent),
            Some(AgentId::ZCode)
        );
        let line = r#"{"jsonrpc":"2.0","method":"initialize","id":1}"#;
        assert_eq!(parse_hello_info(line).map(|i| i.agent), None);
    }

    #[test]
    fn controller_hello_is_distinguished_from_agent_hello() {
        let controller_line = r#"{"bridge":"hello","version":1,"role":"controller","controller":"streamdeck-plugin","instance_id":"p-1","taskSlots":6}"#;
        assert!(crate::plugin_controller::parse_controller_hello(controller_line).is_some());
        // An agent hello must not be mistaken for a controller hello.
        assert!(
            crate::plugin_controller::parse_controller_hello(
                r#"{"bridge":"hello","agent":"codex"}"#
            )
            .is_none()
        );
        // A controller hello with a missing instance_id is rejected.
        assert!(crate::plugin_controller::parse_controller_hello(r#"{"bridge":"hello","role":"controller","controller":"streamdeck-plugin","taskSlots":4}"#).is_none());
    }

    #[test]
    fn versioned_hello_carries_instance_and_focus() {
        let info = parse_hello_info(r#"{"bridge":"hello","version":1,"agent":"zcode","instance_id":"z-1","capabilities":{"focus":true}}"#).unwrap();
        assert_eq!(info.agent, AgentId::ZCode);
        assert_eq!(info.instance_id.as_deref(), Some("z-1"));
        assert!(info.focus_capable);
    }
    #[test]
    fn hello_allows_multiple_sessions_for_same_agent() {
        let mut sessions = vec![SessionHandle {
            id: 1,
            agent: Some(AgentId::Hermes),
            instance_id: Some("one".to_owned()),
            focus_capable: false,
            writer: mpsc::channel().0,
            events: VecDeque::new(),
        }];
        // New session id=2 claims Hermes.
        sessions.push(SessionHandle {
            id: 2,
            agent: None,
            instance_id: None,
            focus_capable: false,
            writer: mpsc::channel().0,
            events: VecDeque::new(),
        });
        handle_hello(
            &mut sessions,
            2,
            HelloInfo {
                agent: AgentId::Hermes,
                instance_id: Some("test".to_owned()),
                focus_capable: true,
            },
        );
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].agent, Some(AgentId::Hermes));
        assert_eq!(sessions[1].id, 2);
        assert_eq!(sessions[1].agent, Some(AgentId::Hermes));
    }
}
