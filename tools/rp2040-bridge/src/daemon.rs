//! Daemon mode: TCP loopback server that multiplexes multiple MCP sessions
//! (Codex, ZCode, and Hermes) over a single hardware-owning bridge process.
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

/// How often to refresh cached usage snapshots (one provider API call per
/// active agent). Defined alongside the usage fetchers in `usage`.
use crate::usage::USAGE_REFRESH_INTERVAL;

/// How often the daemon polls ZCode's session database to mirror live
/// activity on the task board. One second keeps the Stream Deck responsive
/// without hammering the SQLite file.
const ZCODE_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How often the daemon probes for the ZCode desktop app's window. While the
/// app runs, ZCode keeps its deck half and auto-fed cards even without a
/// live MCP proxy; the probe is a window enumeration, so it stays cheap.
const ZCODE_DESKTOP_PROBE_INTERVAL: Duration = Duration::from_secs(2);
/// Hermes' canonical SQLite state is local and WAL-backed. One second keeps
/// cards responsive while avoiding contention with the running agent.
const HERMES_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Codex desktop state is local and cheap to poll. A 250 ms interval keeps
/// focus and lifecycle responsive without reading SQLite in the 10 ms loop.
const CODEX_TASK_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Synthetic owner session for ZCode cards published by the daemon's DB poll.
/// Real MCP sessions are numbered from 1 upward by the acceptor, so this high
/// value never collides in practice.  Presses on these cards are fanned out to
/// every live ZCode session rather than to this id (see `route_task_device_events`).
pub const ZCODE_POLL_SESSION: usize = 999;
/// Synthetic owner for cards mirrored from Hermes' read-only state database.
pub const HERMES_POLL_SESSION: usize = 998;

// Catalog actions without a concrete owning MCP session are routed to the
// newest live session for their intended agent. These sentinels cannot collide
// with daemon connection ids, which increase from one.
const CODEX_CATALOG_SESSION: usize = usize::MAX;
const ZCODE_CATALOG_SESSION: usize = usize::MAX - 1;
const HERMES_CATALOG_SESSION: usize = usize::MAX - 2;

pub(crate) fn catalog_action_session(agent: AgentId) -> usize {
    match agent {
        AgentId::Codex => CODEX_CATALOG_SESSION,
        AgentId::ZCode => ZCODE_CATALOG_SESSION,
        AgentId::Hermes => HERMES_CATALOG_SESSION,
    }
}

fn catalog_action_agent(session_id: usize) -> Option<AgentId> {
    match session_id {
        CODEX_CATALOG_SESSION => Some(AgentId::Codex),
        ZCODE_CATALOG_SESSION => Some(AgentId::ZCode),
        HERMES_CATALOG_SESSION => Some(AgentId::Hermes),
        _ => None,
    }
}

/// Owner session for the synthetic Codex HID cards published from `v.oai.thstatus`.
/// Matches `publish_codex_hid_status`, which hardcodes session 0.
const CODEX_HID_SESSION: usize = 0;

/// How long after the last `v.oai.thstatus` frame the Codex HID cards are kept
/// before they are considered stale (the RP2040 link is gone) and cleared.
const THSTATUS_GRACE: Duration = Duration::from_secs(5);

/// A connected controller only represents active Codex work shortly after it
/// has actually emitted a Codex status frame.
const CODEX_HARDWARE_IDLE: Duration = Duration::from_secs(60);

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
/// without an MCP session), plus ZCode while its desktop app is running
/// (the task feed mirrors the app's database directly and must survive MCP
/// proxy blips and daemon restarts).
fn effective_active_set(
    session_agents: ActiveSet,
    codex_hardware_active: bool,
    zcode_desktop_active: bool,
) -> ActiveSet {
    let mut set = session_agents;
    if codex_hardware_active {
        set.insert(AgentId::Codex);
    }
    if zcode_desktop_active {
        set.insert(AgentId::ZCode);
    }
    set
}

/// Refreshes the usage snapshots the current setup may display: the selected
/// source always; other agents while their sessions are live or while a
/// plugin deck is connected (its strips can render either agent). Returns
/// whether anything was refreshed.
fn refresh_wanted_usage(
    bridge: &mut crate::BridgeRuntime,
    session_agents: &ActiveSet,
    plugin_connected: bool,
) -> bool {
    use crate::usage::UsageAgent;
    let selected = bridge.usage_agent;
    let codex_wanted =
        selected == UsageAgent::Codex || session_agents.contains(AgentId::Codex) || plugin_connected;
    let zcode_wanted =
        selected == UsageAgent::ZCode || session_agents.contains(AgentId::ZCode) || plugin_connected;
    if codex_wanted {
        let snapshot = crate::usage::fetch_usage(UsageAgent::Codex);
        bridge.usage_cache.store(UsageAgent::Codex, snapshot);
    }
    if zcode_wanted {
        let snapshot = crate::usage::fetch_usage(UsageAgent::ZCode);
        bridge.usage_cache.store(UsageAgent::ZCode, snapshot);
    }
    codex_wanted || zcode_wanted
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
    crate::diaglog::log(&format!(
        "daemon starting bind={} port={} controller={}",
        options.bind,
        options.bridge_options.port,
        options.bridge_options.controller.as_str()
    ));
    let listener = TcpListener::bind(&options.bind)
        .map_err(|error| format!("failed to bind {}: {error}", options.bind))?;
    let local = listener
        .local_addr()
        .map_err(|error| format!("could not resolve local addr: {error}"))?;
    eprintln!("daemon [{}] listening on {local}", log_context());

    let mut bridge = crate::open_runtime(&options.bridge_options)?;
    bridge.task_mode = true;
    let mut codex_snapshot_available = false;
    if let Some(snapshot) =
        crate::codex_state::read_codex_snapshot(crate::tasks::CODEX_TASK_SLOTS)
    {
        match bridge
            .task_board
            .publish_codex_snapshot(&snapshot, now_ms())
        {
            Ok(_) => {
                bridge.has_explicit_task_state = true;
                codex_snapshot_available = true;
            }
            Err(error) => eprintln!("Codex state snapshot rejected: {error}"),
        }
    }
    let mut next_codex_poll_at = Instant::now() + CODEX_TASK_POLL_INTERVAL;
    // A connected RP2040 alone does not claim a share of the deck.
    let mut session_agents = ActiveSet::new();
    let mut codex_hardware_active = false;
    // Probed periodically: while the ZCode desktop app runs, ZCode keeps its
    // half of the deck and its auto-fed cards even without a live MCP proxy.
    let mut zcode_desktop_active = crate::zcode_window::desktop_running();
    if zcode_desktop_active {
        crate::diaglog::log("zcode desktop app detected at daemon start");
    }
    let mut next_zcode_desktop_probe_at = Instant::now() + ZCODE_DESKTOP_PROBE_INTERVAL;
    let mut pending_repartition_at: Option<Instant> = None;
    let mut next_usage_refresh_at: Option<Instant> = None;
    {
        let active = effective_active_set(session_agents, codex_hardware_active, zcode_desktop_active);
        bridge.partition = Partition::compute(active);
        bridge.task_board.set_slot_owners(
            bridge.task_device_id.clone(),
            (0..bridge.task_slot_count)
                .map(|slot| bridge.partition.owner_of(slot as u8))
                .collect(),
        );
    }
    eprintln!("{}", crate::bridge_status(&bridge, "daemon"));

    let (session_tx, session_rx) = mpsc::channel::<SessionMessage>();
    let mut sessions: Vec<SessionHandle> = Vec::new();
    let _next_session_id: usize = 1;

    // Pending long-polls: one per agent. Each entry holds the session id and
    // the deadline until which the daemon should wait before responding with
    // an empty result.
    let mut pending_polls: Vec<PendingPoll> = Vec::new();
    let mut serial_retry_at =
        (!bridge.has_serial() && options.bridge_options.port != "none").then(Instant::now);
    let mut serial_retry_delay = crate::SERIAL_RETRY_INITIAL_DELAY;

    // Plugin controller sessions: maps session_id to the device_id of the
    // PluginController registered in aux_controllers.
    let mut plugin_sessions: Vec<(usize, String)> = Vec::new();

    let mut last_thstatus_at: Option<Instant> = None;
    // ZCode and Hermes auto-feed bookkeeping.  The daemon periodically polls
    // their on-disk session databases and publishes active sessions as task
    // cards, mirroring how Codex activity arrives via `v.oai.thstatus`.
    let mut next_zcode_poll_at: Option<Instant> = None;
    let mut next_hermes_poll_at: Option<Instant> = None;

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
                    // Codex owns the CLI/session-derived fallback context; a
                    // ZCode hello also warrants re-derivation so a ZCode-only
                    // machine still populates the strip (and its usage data).
                    if matches!(agent, AgentId::Codex | AgentId::ZCode) {
                        if let Some(usage_agent) = crate::usage::UsageAgent::from_agent(agent) {
                            let snapshot = crate::usage::fetch_usage(usage_agent);
                            bridge.usage_cache.store(usage_agent, snapshot);
                        }
                        crate::auto_derive_display_context(&mut bridge);
                        let _ = crate::refresh_task_board(&mut bridge);
                    }
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
                    // A reconnect reuses the plugin process instance id. Remove
                    // the old mapping before installing its replacement so a
                    // late disconnect from the stale socket cannot detach both.
                    let replaced =
                        remove_plugin_sessions_by_device(&mut plugin_sessions, &device_id);
                    if !replaced.is_empty() {
                        eprintln!(
                            "plugin controller {device_id} replacing stale sessions {replaced:?}"
                        );
                        detach_plugin_controller(&mut bridge, &device_id);
                    }

                    let slots = controller.task_slot_count();
                    // With --controller none, the first Stream Deck plugin is
                    // the physical deck. Promote it to the partitioned primary
                    // instead of letting task assignment drift between plugin
                    // instances.
                    let becomes_primary =
                        bridge.controller.is_none() && bridge.task_device_id == "none";
                    if becomes_primary {
                        bridge.task_device_id = device_id.clone();
                        bridge.task_slot_count = slots;
                    }
                    if slots > 0 {
                        bridge.task_board.set_device(device_id.clone(), slots, true);
                        if becomes_primary {
                            bridge.task_board.set_slot_owners(
                                device_id.clone(),
                                (0..slots)
                                    .map(|slot| bridge.partition.owner_of(slot as u8))
                                    .collect(),
                            );
                        }
                    }
                    bridge
                        .aux_controllers
                        .push((device_id.clone(), Box::new(controller), slots));
                    plugin_sessions.push((session_id, device_id.clone()));
                    // Bootstrap usage data for the deck right away (both
                    // agents) instead of waiting for the first periodic tick.
                    if refresh_wanted_usage(&mut bridge, &session_agents, true) {
                        if bridge.has_explicit_display_context {
                            crate::patch_display_context_usage(&mut bridge);
                        } else {
                            crate::auto_derive_display_context(&mut bridge);
                        }
                    }
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
                                    let method = crate::codex::method(&message);
                                    if bridge.task_mode && method == Some("v.oai.thstatus") {
                                        if let Some(parameters) =
                                            message.get("p").or_else(|| message.get("params"))
                                        {
                                            // The serial status is an explicit Codex update even
                                            // when an optional task field cannot be adapted. Keep
                                            // it authoritative and let the renderer fall back to
                                            // the fused status buffer rather than standby cards.
                                            bridge.has_explicit_task_state = true;
                                            last_thstatus_at = Some(Instant::now());
                                            if !codex_snapshot_available {
                                                match bridge
                                                    .task_board
                                                    .publish_codex_hid_status(parameters, now_ms())
                                                {
                                                    Ok(()) => eprintln!(
                                                        "Codex HID fallback accepted entries={}",
                                                        parameters
                                                            .as_array()
                                                            .map(Vec::len)
                                                            .unwrap_or(0)
                                                    ),
                                                    Err(error) => eprintln!(
                                                        "Codex HID fallback rejected: {error}"
                                                    ),
                                                }
                                            }
                                            crate::auto_derive_display_context(&mut bridge);
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

        // Refresh authoritative Codex task identity, metadata, focus, lifecycle,
        // and exact timer timestamps independently of LCD presentation frames.
        if now >= next_codex_poll_at {
            next_codex_poll_at = now + CODEX_TASK_POLL_INTERVAL;
            // Match the ZCode/Hermes feeds: publish only as many recent
            // threads as Codex can display on its partition slots.
            let codex_slots = bridge.partition.slots_for(AgentId::Codex).len();
            if let Some(snapshot) =
                crate::codex_state::read_codex_snapshot(codex_slots)
            {
                match bridge
                    .task_board
                    .publish_codex_snapshot(&snapshot, now_ms())
                {
                    Ok(_) => {
                        bridge.has_explicit_task_state = true;
                        codex_snapshot_available = true;
                    }
                    Err(error) => eprintln!("Codex state snapshot rejected: {error}"),
                }
            }
        }

        // Controller reconnect + poll.
        crate::reconnect_controller_if_due(&mut bridge);
        bridge.task_board.expire(now_ms());
        let _ = crate::refresh_task_board(&mut bridge);
        crate::poll_controller(&mut bridge, false)?;
        for (session_id, event) in bridge.pending_task_events.drain(..) {
            if let Some(agent) = catalog_action_agent(session_id) {
                // Multiple proxies for one agent can briefly overlap during a
                // restart. Deliver one command to the newest session only so
                // a single Stream Deck press never executes twice.
                let target = sessions
                    .iter()
                    .filter(|session| session.agent == Some(agent))
                    .max_by_key(|session| session.id)
                    .map(|session| session.id);
                if let Some(target) = target {
                    push_session_event(&mut sessions, target, event);
                }
            } else if session_id == ZCODE_POLL_SESSION {
                // The auto-fed ZCode cards have no owning MCP session of their
                // own; fan a press out to every live ZCode session so the agent
                // receives the `task_selected` (and legacy key) event.
                let targets: Vec<usize> = sessions
                    .iter()
                    .filter(|session| session.agent == Some(AgentId::ZCode))
                    .map(|session| session.id)
                    .collect();
                for id in targets {
                    if let Some(session) = sessions.iter_mut().find(|session| session.id == id) {
                        session.events.push_back(event.clone());
                    }
                }
            } else if session_id == HERMES_POLL_SESSION {
                let targets: Vec<usize> = sessions
                    .iter()
                    .filter(|session| session.agent == Some(AgentId::Hermes))
                    .map(|session| session.id)
                    .collect();
                for id in targets {
                    if let Some(session) = sessions.iter_mut().find(|session| session.id == id) {
                        session.events.push_back(event.clone());
                    }
                }
            } else if let Some(session) =
                sessions.iter_mut().find(|session| session.id == session_id)
            {
                session.events.push_back(event);
            }
        }
        // A task-button press during poll may have changed the selected
        // task; push the updated display context and cards to aux
        // controllers so the strips reflect the new selection immediately.
        let _ = crate::refresh_task_board(&mut bridge);

        // Clear phantom Codex HID cards when the RP2040 link is gone.  The
        // synthetic session 0 cards published from `v.oai.thstatus` never
        // expire on their own; without this they keep hogging slots 0-5 and
        // starve the ZCode and Hermes auto-feeds below.
        // Status is event-driven, not a heartbeat. Do not clear active task
        // cards simply because no new status frame arrived while the serial
        // link remains healthy.
        let thstatus_stale = !bridge.has_serial()
            && last_thstatus_at
                .map(|seen| now.duration_since(seen) > THSTATUS_GRACE)
                .unwrap_or(true);
        if thstatus_stale
            && !codex_snapshot_available
            && bridge.task_board.has_session_tasks(CODEX_HID_SESSION)
        {
            bridge.task_board.clear_session(CODEX_HID_SESSION);
            if !bridge.task_board.has_tasks() {
                bridge.has_explicit_task_state = false;
            }
            let _ = crate::refresh_task_board(&mut bridge);
        }

        // ZCode auto-feed mirrors its on-disk state while a ZCode proxy is
        // live or the desktop app is running. ZCode retains historical
        // sessions in its database, so polling with neither present
        // resurrects old tasks and can take over the board. An explicit
        // ZCode publication is authoritative and suppresses this synthetic
        // owner until those manual cards disconnect and expire.
        let zcode_proxy_active = sessions
            .iter()
            .any(|session| session.agent == Some(AgentId::ZCode));
        let zcode_active = zcode_proxy_active || zcode_desktop_active;
        let zcode_has_manual_tasks = bridge
            .task_board
            .has_agent_tasks_except(AgentId::ZCode, ZCODE_POLL_SESSION);
        if !zcode_active || zcode_has_manual_tasks {
            if bridge.task_board.has_session_tasks(ZCODE_POLL_SESSION) {
                bridge.task_board.clear_session(ZCODE_POLL_SESSION);
                crate::diaglog::log(&format!(
                    "zcode auto-feed cleared (proxy={}, desktop={}, manual={})",
                    zcode_proxy_active, zcode_desktop_active, zcode_has_manual_tasks
                ));
                let _ = crate::refresh_task_board(&mut bridge);
            }
            next_zcode_poll_at = None;
        } else {
            if next_zcode_poll_at.is_none() {
                next_zcode_poll_at = Some(now + ZCODE_POLL_INTERVAL);
                crate::diaglog::log(&format!(
                    "zcode auto-feed active (proxy={}, desktop={})",
                    zcode_proxy_active, zcode_desktop_active
                ));
            }
            if let Some(poll_at) = next_zcode_poll_at {
                if now >= poll_at {
                    next_zcode_poll_at = Some(now + ZCODE_POLL_INTERVAL);
                    // Publish only as many recent sessions as ZCode can display
                    // on the partitioned primary controller. Publishing a fixed
                    // six-card snapshot while ZCode owns three slots leaves old
                    // sticky assignments in place and sends newly started tasks
                    // to overflow with no visible change on the device.
                    let zcode_slots = bridge.partition.slots_for(AgentId::ZCode).len();
                    if let Some(snapshot) =
                        crate::zcode_state::read_zcode_snapshot(now_ms(), zcode_slots)
                    {
                        match bridge.task_board.publish_tasks(
                            ZCODE_POLL_SESSION,
                            AgentId::ZCode,
                            &snapshot,
                            now_ms(),
                        ) {
                            Ok(_) => {
                                bridge.has_explicit_task_state = true;
                                let _ = crate::refresh_task_board(&mut bridge);
                            }
                            Err(error) => {
                                eprintln!("zcode poll publish failed: {error}");
                                crate::diaglog::log(&format!(
                                    "zcode auto-feed publish failed: {error}"
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Mirror Hermes' canonical state only while its proxy is live. An
        // explicit Hermes publication is authoritative and suppresses this
        // synthetic owner until those manual cards disconnect and expire.
        let hermes_active = sessions
            .iter()
            .any(|session| session.agent == Some(AgentId::Hermes));
        let hermes_has_manual_tasks = bridge
            .task_board
            .has_agent_tasks_except(AgentId::Hermes, HERMES_POLL_SESSION);
        if !hermes_active || hermes_has_manual_tasks {
            if bridge.task_board.has_session_tasks(HERMES_POLL_SESSION) {
                bridge.task_board.clear_session(HERMES_POLL_SESSION);
                let _ = crate::refresh_task_board(&mut bridge);
            }
            next_hermes_poll_at = None;
        } else {
            if next_hermes_poll_at.is_none() {
                next_hermes_poll_at = Some(now);
            }
            if next_hermes_poll_at.is_some_and(|poll_at| now >= poll_at) {
                next_hermes_poll_at = Some(now + HERMES_POLL_INTERVAL);
                let hermes_slots = bridge.partition.slots_for(AgentId::Hermes).len();
                if let Some(snapshot) =
                    crate::hermes_state::read_hermes_snapshot(now_ms(), hermes_slots)
                {
                    match bridge.task_board.publish_tasks(
                        HERMES_POLL_SESSION,
                        AgentId::Hermes,
                        &snapshot,
                        now_ms(),
                    ) {
                        Ok(_) => {
                            bridge.has_explicit_task_state = true;
                            let _ = crate::refresh_task_board(&mut bridge);
                        }
                        Err(error) => eprintln!("hermes poll publish failed: {error}"),
                    }
                }
            }
        }

        // A serial link reserves Codex slots only while recent status traffic is present.
        let now_codex_hardware_active = bridge.has_serial()
            && last_thstatus_at.is_some_and(|seen| now.duration_since(seen) < CODEX_HARDWARE_IDLE);
        if now_codex_hardware_active != codex_hardware_active {
            codex_hardware_active = now_codex_hardware_active;
            pending_repartition_at = Some(now + REPARTITION_DEBOUNCE);
        }

        // The ZCode desktop app running is itself an activity signal: the
        // auto-feed mirrors its database directly, so cards must not blink
        // out just because the MCP proxy reconnected or the daemon restarted.
        if now >= next_zcode_desktop_probe_at {
            next_zcode_desktop_probe_at = now + ZCODE_DESKTOP_PROBE_INTERVAL;
            let running = crate::zcode_window::desktop_running();
            if running != zcode_desktop_active {
                zcode_desktop_active = running;
                crate::diaglog::log(&format!(
                    "zcode desktop app {}",
                    if running { "detected" } else { "no longer running" }
                ));
                pending_repartition_at = Some(now + REPARTITION_DEBOUNCE);
            }
        }

        // Apply a debounced repartition if the active set has changed.
        if let Some(repartition_at) = pending_repartition_at {
            if now >= repartition_at {
                pending_repartition_at = None;
                let active = effective_active_set(
                    session_agents,
                    codex_hardware_active,
                    zcode_desktop_active,
                );
                let new_partition = Partition::compute(active);
                eprintln!(
                    "repartition: active={:?} owners={:?}",
                    active.iter(),
                    new_partition.owners_json()
                );
                crate::diaglog::log(&format!(
                    "repartition applied: active={:?} owners={:?}",
                    active.iter(),
                    new_partition.owners_json()
                ));
                bridge.partition = new_partition.clone();
                bridge.task_board.set_slot_owners(
                    bridge.task_device_id.clone(),
                    (0..bridge.task_slot_count)
                        .map(|slot| new_partition.owner_of(slot as u8))
                        .collect(),
                );
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

        // Periodically refresh cached usage snapshots so the 5-hour and
        // weekly remaining figures stay current. The selected agent is
        // always refreshed; other agents while their sessions are live or
        // while a plugin deck is attached (its strips can render either
        // agent), which keeps later source switches instant.
        if next_usage_refresh_at.is_none() {
            next_usage_refresh_at = Some(now + USAGE_REFRESH_INTERVAL);
        }
        if let Some(refresh_at) = next_usage_refresh_at {
            if now >= refresh_at {
                next_usage_refresh_at = Some(now + USAGE_REFRESH_INTERVAL);
                let plugin_connected = !plugin_sessions.is_empty();
                if refresh_wanted_usage(&mut bridge, &session_agents, plugin_connected) {
                    if bridge.has_explicit_display_context {
                        crate::patch_display_context_usage(&mut bridge);
                    } else {
                        crate::auto_derive_display_context(&mut bridge);
                    }
                    let _ = crate::refresh_task_board(&mut bridge);
                }
            }
        }

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
        crate::diaglog::log(&format!(
            "agent session {} registered: {} (instance {}, focus={})",
            session_id,
            info.agent.as_str(),
            session.instance_id.as_deref().unwrap_or("unknown"),
            session.focus_capable
        ));
    }
}

fn remove_session(sessions: &mut Vec<SessionHandle>, session_id: usize) {
    if let Some(session) = sessions.iter().find(|s| s.id == session_id) {
        crate::diaglog::log(&format!(
            "agent session {} removed: {} (instance {})",
            session_id,
            session.agent.as_ref().map_or("none", |a| a.as_str()),
            session.instance_id.as_deref().unwrap_or("unknown")
        ));
    }
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

/// Removes every stale session mapping for a reconnecting plugin device.
fn remove_plugin_sessions_by_device(
    plugin_sessions: &mut Vec<(usize, String)>,
    device_id: &str,
) -> Vec<usize> {
    let mut removed = Vec::new();
    plugin_sessions.retain(|(session_id, candidate)| {
        if candidate.as_str() == device_id {
            removed.push(*session_id);
            false
        } else {
            true
        }
    });
    removed
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
        // Plugin instance IDs include the Node process ID and change whenever
        // Stream Deck restarts. Forget a disconnected primary so the next
        // plugin hello can become the partitioned physical deck.
        if bridge.task_device_id == device_id {
            bridge.task_device_id = "none".to_owned();
            bridge.task_slot_count = 0;
            bridge.task_board.set_slot_owners("none", Vec::new());
        }
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
                 Use poll_events to receive key presses, partition changes, and task-card events. \
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
    // `Value::Null` is the deferred-response sentinel used by long polls:
    // `resolve_pending_polls` answers that request id later, so replying now
    // would leak a null result and later duplicate the response.
    if !result.is_null() {
        send_to_session(sessions, session_id, mcp::response(id, result));
    }
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
            let auto_feed_session = match agent_id {
                AgentId::ZCode => Some(ZCODE_POLL_SESSION),
                AgentId::Hermes => Some(HERMES_POLL_SESSION),
                AgentId::Codex => None,
            };
            if let Some(poll_session) = auto_feed_session.filter(|&s| s != session_id) {
                // Avoid duplicate stable IDs when the first explicit snapshot
                // takes ownership from the read-only auto-feed.
                bridge.task_board.clear_session(poll_session);
            }
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
            // Drain both sources in one poll: session events (task-card and
            // layout notifications) and the agent's routing queue (physical
            // key presses and partition changes). Returning only one leaves
            // the other buffered and can starve key events.
            let mut events = drain_session_events(sessions, session_id);
            events.extend(
                bridge
                    .routing
                    .queue_mut(agent)
                    .drain()
                    .into_iter()
                    .map(|event| event.to_json()),
            );
            if !events.is_empty() {
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
                // Null sentinel: `handle_request` must not answer this id;
                // `resolve_pending_polls` replies for it later.
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
    // forwarded (ZCode and Hermes slots are not part of the Codex Micro
    // protocol).
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
        object.insert("usageAgent".to_owned(), json!(bridge.usage_agent.as_str()));
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
            // Merge both event sources so neither starves the other
            // (mirrors the immediate poll_events path).
            let mut events = drain_session_events(sessions, poll.session_id);
            events.extend(
                bridge
                    .routing
                    .queue_mut(poll.agent)
                    .drain()
                    .into_iter()
                    .map(|e| e.to_json())
                    .collect::<Vec<_>>(),
            );
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
    fn reconnect_removes_only_stale_device_sessions() {
        let mut mappings = vec![
            (10, "deck:a".to_owned()),
            (11, "deck:b".to_owned()),
            (12, "deck:a".to_owned()),
        ];
        let removed = remove_plugin_sessions_by_device(&mut mappings, "deck:a");
        assert_eq!(removed, vec![10, 12]);
        assert_eq!(mappings, vec![(11, "deck:b".to_owned())]);
    }

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
    fn idle_serial_does_not_add_phantom_codex() {
        let mut sessions = ActiveSet::new();
        sessions.insert(AgentId::ZCode);
        sessions.insert(AgentId::Hermes);

        let active = effective_active_set(sessions, false, false);
        assert!(!active.contains(AgentId::Codex));
        assert_eq!(
            Partition::compute(active).owners_json(),
            vec![
                Value::Null,
                Value::Null,
                Value::Null,
                json!("zcode"),
                json!("zcode"),
                json!("zcode")
            ]
        );
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
