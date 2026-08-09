mod ajazz;
mod codex;
mod controller;
mod daemon;
mod mcp;
mod plugin_controller;
mod proxy;
mod routing;
mod serial;
mod streamdeck;
mod tasks;
mod wire;

use crate::codex::{CodexDecoder, RadialState, frame_json, messages_for_synthetic_key};
use crate::controller::{ControllerKind, DeviceSpec, DisplayContext, PhysicalController};
use crate::serial::SerialEvent;
use crate::streamdeck::connect as connect_streamdeck;
use crate::wire::{Frame, FrameType};
use serde_json::{Value, json};
use std::env;
use std::fs::File;
use std::io::{self, IsTerminal};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(1);
const CONTROLLER_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(250);
const CONTROLLER_RETRY_MAX_DELAY: Duration = Duration::from_secs(1);
pub(crate) const SERIAL_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(100);
pub(crate) const SERIAL_RETRY_MAX_DELAY: Duration = Duration::from_secs(1);

pub struct Options {
    pub port: String,
    pub controller: ControllerKind,
    pub controller_serial: Option<String>,
    pub devices: Vec<DeviceSpec>,
    pub listen_seconds: Option<u64>,
    pub emit_key: Option<String>,
    pub emit_after_seconds: u64,
    pub mcp: bool,
    pub legacy: bool,
    pub daemon: bool,
    pub bind: String,
    pub mcp_proxy: bool,
    pub agent: Option<crate::routing::AgentId>,
    pub connect: String,
    pub autostart: bool,
    pub daemon_args: Vec<String>,
}

fn parse_options() -> Result<Options, String> {
    let mut port = None;
    let mut no_ajazz = false;
    let mut controller = None;
    let mut controller_serial = None;
    let mut device_specs: Vec<DeviceSpec> = Vec::new();
    let mut listen_seconds = None;
    let mut emit_key = None;
    let mut emit_after_seconds = 3;
    let mut mcp = false;
    let mut legacy = false;
    let mut daemon = false;
    let mut bind = crate::daemon::DEFAULT_BIND.to_owned();
    let mut mcp_proxy = false;
    let mut agent = None;
    let mut connect = crate::daemon::DEFAULT_BIND.to_owned();
    let mut autostart = false;
    let mut daemon_args = Vec::new();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--" => {}
            "--port" => {
                port = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--port requires COMx, auto, or none".to_owned())?,
                );
            }
            "--no-ajazz" => no_ajazz = true,
            "--controller" => {
                if controller.is_some() {
                    return Err("--controller may only be specified once".to_owned());
                }
                controller =
                    Some(ControllerKind::parse(&arguments.next().ok_or_else(
                        || "--controller requires a controller kind".to_owned(),
                    )?)?);
            }
            "--controller-serial" => {
                controller_serial =
                    Some(arguments.next().ok_or_else(|| {
                        "--controller-serial requires a serial number".to_owned()
                    })?);
            }
            "--device" => {
                let value = arguments.next().ok_or_else(|| {
                    "--device requires KIND[,serial=SERIAL][,task-slots=N]".to_owned()
                })?;
                device_specs.push(DeviceSpec::parse(&value)?);
            }
            "--listen" => {
                let seconds = arguments
                    .next()
                    .ok_or_else(|| "--listen requires seconds".to_owned())?
                    .parse::<u64>()
                    .map_err(|_| "--listen must be an integer".to_owned())?;
                if seconds > 3600 {
                    return Err(
                        "--listen must be from 0 to 3600 seconds (0 = unlimited)".to_owned()
                    );
                }
                listen_seconds = (seconds > 0).then_some(seconds);
            }
            "--emit" => {
                emit_key = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--emit requires a Codex Micro key".to_owned())?,
                );
            }
            "--emit-after" => {
                emit_after_seconds = arguments
                    .next()
                    .ok_or_else(|| "--emit-after requires seconds".to_owned())?
                    .parse::<u64>()
                    .map_err(|_| "--emit-after must be an integer".to_owned())?;
                if emit_after_seconds > 3600 {
                    return Err("--emit-after must be from 0 to 3600 seconds".to_owned());
                }
            }
            "--mcp" => mcp = true,
            "--legacy" => legacy = true,
            "--daemon" => daemon = true,
            "--bind" => {
                bind = arguments
                    .next()
                    .ok_or_else(|| "--bind requires an address".to_owned())?;
            }
            "--mcp-proxy" => mcp_proxy = true,
            "--agent" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--agent requires codex, zcode, or hermes".to_owned())?;
                agent = Some(crate::routing::AgentId::parse(&value)?);
            }
            "--connect" => {
                connect = arguments
                    .next()
                    .ok_or_else(|| "--connect requires an address".to_owned())?;
            }
            "--autostart" => autostart = true,
            "--daemon-args" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--daemon-args requires a string".to_owned())?;
                daemon_args = value.split_whitespace().map(str::to_owned).collect();
            }
            "--help" | "-h" => {
                println!(
                    "rp2040-bridge --port COMx|auto|none [--device KIND[,serial=SERIAL][,task-slots=N]]... [--controller ajazz|streamdeck-plus|streamdeck-plus-xl|streamdeck-xl|none] [--controller-serial SERIAL] [--no-ajazz] [--listen 0..3600] [--emit AG00] [--emit-after 0..3600] [--mcp|--legacy|--daemon [--bind 127.0.0.1:48360] | --mcp-proxy --agent codex|zcode|hermes [--connect 127.0.0.1:48360] [--autostart] [--daemon-args \"...\"]]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    let mode_count = [mcp, legacy, daemon, mcp_proxy]
        .iter()
        .filter(|&&f| f)
        .count();
    if mode_count > 1 {
        return Err("--mcp, --legacy, --daemon, and --mcp-proxy are mutually exclusive".to_owned());
    }
    if !device_specs.is_empty() {
        if no_ajazz || controller.is_some() || controller_serial.is_some() {
            return Err(
                "--device cannot be combined with --no-ajazz, --controller, or --controller-serial"
                    .to_owned(),
            );
        }
    } else {
        let selected = controller.unwrap_or(if no_ajazz {
            ControllerKind::None
        } else {
            ControllerKind::Ajazz
        });
        if controller_serial.is_some() && !selected.is_physical() {
            return Err("--controller-serial requires a physical --controller".to_owned());
        }
        if selected.is_physical() {
            device_specs.push(DeviceSpec {
                kind: selected,
                serial: controller_serial.clone(),
                task_slots: None,
            });
        }
    }
    let controller = device_specs
        .first()
        .map(|spec| spec.kind)
        .unwrap_or(ControllerKind::None);
    let controller_serial = device_specs.first().and_then(|spec| spec.serial.clone());
    if mcp_proxy && agent.is_none() {
        return Err("--mcp-proxy requires --agent codex|zcode|hermes".to_owned());
    }
    if let Some(key) = &emit_key {
        messages_for_synthetic_key(key)?;
        if port.as_deref() == Some("none") {
            return Err("--emit cannot be used with --port none".to_owned());
        }
    }
    // The proxy mode does not need a port; it connects to the daemon.
    let port = if mcp_proxy {
        String::from("proxy")
    } else {
        port.unwrap_or_else(|| "auto".to_owned())
    };
    Ok(Options {
        port,
        controller,
        controller_serial,
        devices: device_specs,
        listen_seconds,
        emit_key,
        emit_after_seconds,
        mcp,
        legacy,
        daemon,
        bind,
        mcp_proxy,
        agent,
        connect,
        autostart,
        daemon_args,
    })
}

fn next_sequence(sequence: &mut u16) -> u16 {
    let value = *sequence;
    *sequence = sequence.wrapping_add(1);
    value
}

fn send_codex_message(
    writer: &mut File,
    sequence: &mut u16,
    message: &Value,
) -> Result<(), String> {
    for report in frame_json(message)? {
        let frame = Frame::new(
            FrameType::CodexInputReport,
            next_sequence(sequence),
            report.to_vec(),
        )
        .map_err(|error| error.to_string())?;
        serial::write_frame(writer, &frame)?;
    }
    Ok(())
}

fn wait_for_firmware(
    receiver: &Receiver<SerialEvent>,
    writer: &mut File,
    sequence: &mut u16,
) -> Result<String, String> {
    let ping = Frame::new(FrameType::Ping, next_sequence(sequence), Vec::new())
        .map_err(|error| error.to_string())?;
    serial::write_frame(writer, &ping)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("RP2040 did not answer the bridge ping within 3 seconds".to_owned());
        }
        match receiver.recv_timeout(remaining) {
            Ok(SerialEvent::Frame(frame)) if frame.frame_type == FrameType::Status => {
                return String::from_utf8(frame.payload)
                    .map_err(|_| "RP2040 returned a non-UTF8 status".to_owned());
            }
            Ok(SerialEvent::ProtocolError(error)) => return Err(error),
            Ok(SerialEvent::Disconnected(error)) => return Err(error),
            Ok(SerialEvent::Frame(_)) => {}
            Err(RecvTimeoutError::Timeout) => {
                return Err("RP2040 did not answer the bridge ping within 3 seconds".to_owned());
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err("RP2040 serial reader stopped".to_owned());
            }
        }
    }
}

fn rpc_ack_response(message: &Value) -> Option<Value> {
    match codex::method(message) {
        Some("v.oai.rgbcfg") | Some("v.oai.thstatus") => message
            .get("id")
            .map(|id| json!({"result": true, "id": id})),
        _ => None,
    }
}

#[derive(Debug)]
pub enum ProcessError {
    Controller(String),
    Protocol(String),
    Serial(String),
}

pub fn process_codex_message(
    message: Value,
    controller: &mut Option<Box<dyn PhysicalController>>,
    last_thread_status: &mut Option<Value>,
    last_rgb_config: &mut Option<Value>,
    fused_lcd: &mut crate::routing::FusedLcdState,
    partition: &crate::routing::Partition,
    writer: Option<&mut File>,
    sequence: &mut u16,
    trace: bool,
) -> Result<(), ProcessError> {
    let method = codex::method(&message).map(str::to_owned);
    match method.as_deref() {
        Some("device.status") => {}
        Some("v.oai.thstatus") => {
            let parameters = message.get("p").or_else(|| message.get("params"));
            if let Some(parameters) = parameters {
                *last_thread_status = Some(parameters.clone());
                // ChatGPT (HID) sends thstatus as Codex. Merge into Codex's
                // local buffer and render through the current partition.
                let fused = match fused_lcd.merge_from_agent(
                    crate::routing::AgentId::Codex,
                    parameters,
                    partition,
                ) {
                    Ok(fused) => fused,
                    Err(error) if !parameters.is_array() => {
                        return Err(ProcessError::Protocol(error));
                    }
                    Err(error) => return Err(ProcessError::Protocol(error)),
                };
                if let Some(device) = controller {
                    let fused_value = Value::Array(fused);
                    match device.apply_thread_status(&fused_value) {
                        Ok(()) => {}
                        Err(error) if !parameters.is_array() => {
                            return Err(ProcessError::Protocol(error));
                        }
                        Err(error) => return Err(ProcessError::Controller(error)),
                    }
                }
            }
        }
        Some("v.oai.rgbcfg") => {
            let parameters = message.get("p").or_else(|| message.get("params"));
            if let Some(parameters) = parameters {
                *last_rgb_config = Some(parameters.clone());
                if let Some(device) = controller {
                    match device.apply_rgb_config(parameters) {
                        Ok(()) => {}
                        Err(error) if !parameters.is_object() => {
                            return Err(ProcessError::Protocol(error));
                        }
                        Err(error) => return Err(ProcessError::Controller(error)),
                    }
                }
            }
        }
        _ => {}
    }
    if let Some(response) = rpc_ack_response(&message) {
        if let Some(writer) = writer {
            send_codex_message(writer, sequence, &response).map_err(ProcessError::Serial)?;
        }
        if trace {
            println!(
                "{}",
                json!({"type":"codex-response","method":method,"id":message.get("id"),"result":true})
            );
        }
    }
    if trace {
        println!(
            "{}",
            json!({"type":"codex-message","method":method,"id":message.get("id")})
        );
    }
    Ok(())
}
fn codex_report_trace(report: &[u8], sequence: u16) -> Value {
    let report_id = report.first().copied().unwrap_or_default();
    let opcode = report.get(1).copied().unwrap_or_default();
    let declared_length = report.get(2).copied().unwrap_or_default() as usize;
    let available_length = report.len().saturating_sub(3);
    let payload_length = declared_length.min(available_length);
    let prefix_length = payload_length.min(16);
    let payload_prefix_hex = report
        .get(3..3 + prefix_length)
        .unwrap_or_default()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    json!({
        "type": "codex-report",
        "direction": "host-to-bridge",
        "sequence": sequence,
        "reportBytes": report.len(),
        "reportId": report_id,
        "opcode": opcode,
        "declaredChunkBytes": declared_length,
        "payloadPrefixHex": payload_prefix_hex
    })
}

pub struct SerialRuntime {
    pub writer: Option<File>,
    pub receiver: Receiver<SerialEvent>,
    pub reader_thread: Option<JoinHandle<()>>,
}

pub struct BridgeRuntime {
    pub serial: Option<SerialRuntime>,
    pub sequence: u16,
    pub controller: Option<Box<dyn PhysicalController>>,
    pub aux_controllers: Vec<(String, Box<dyn PhysicalController>, usize)>,
    pub controller_choice: ControllerKind,
    pub controller_serial: Option<String>,
    pub devices: Vec<DeviceSpec>,
    pub controller_retry_at: Instant,
    pub controller_retry_delay: Duration,
    pub last_thread_status: Option<Value>,
    pub has_explicit_task_state: bool,
    pub last_rgb_config: Option<Value>,
    pub last_display_context: Option<DisplayContext>,
    /// Whether `last_display_context` was set by an explicit MCP
    /// `set_display_context` call (true) or auto-derived from thstatus
    /// data (false). Auto-derived context is overwritten by the next
    /// thstatus update; explicit context persists until the next call.
    pub has_explicit_display_context: bool,
    pub firmware: String,
    pub port: String,
    pub codex_decoder: CodexDecoder,
    pub radial_state: RadialState,
    pub health: HealthCheck,
    pub routing: crate::routing::EventRouting,
    pub fused_lcd: crate::routing::FusedLcdState,
    pub partition: crate::routing::Partition,
    pub task_board: crate::tasks::TaskBoard,
    pub task_mode: bool,
    pub task_device_id: String,
    pub task_slot_count: usize,
    pub pending_task_events: Vec<(usize, Value)>,
}

impl BridgeRuntime {
    fn has_serial(&self) -> bool {
        self.serial.is_some()
    }

    /// Sends a Codex Micro JSON message through the serial port, if present.
    /// In standalone mode this is a no-op.
    fn send_codex(&mut self, message: &Value) -> Result<(), String> {
        let Some(writer) = self
            .serial
            .as_mut()
            .and_then(|runtime| runtime.writer.as_mut())
        else {
            return Ok(());
        };
        send_codex_message(writer, &mut self.sequence, message)
    }
}

fn connection_default_cards(slot_count: usize) -> Vec<Value> {
    const COLORS: [u32; 8] = [
        0x1565c0, 0x00897b, 0x6a1b9a, 0xef6c00, 0x2e7d32, 0xad1457, 0x0277bd, 0x5d4037,
    ];
    let active_slots = slot_count.min(8);
    // Always return the complete physical card set. Explicitly disabling the
    // unused tail prevents a previous eight-card standby frame from surviving
    // when Codex is assigned only six logical slots.
    (0..8)
        .map(|id| {
            let enabled = id < active_slots;
            json!({
                "id": id,
                "e": u8::from(enabled),
                "c": COLORS[id],
                "b": if enabled { 0.70 } else { 0.0 },
                "agent": "codex"
            })
        })
        .collect()
}

fn connection_default_context() -> DisplayContext {
    DisplayContext {
        project: Some("MICRO-EMU".to_owned()),
        task: Some("BRIDGE".to_owned()),
        model: Some("CODEX".to_owned()),
        effort: Some("LIVE".to_owned()),
        status: Some("READY".to_owned()),
        progress: Some(0),
        task_id: None,
        weekly_remaining: None,
        five_hour_remaining: None,
    }
}

fn desired_primary_cards(bridge: &BridgeRuntime) -> Vec<Value> {
    if bridge.task_mode {
        // A v.oai.thstatus message is explicit state even when it arrived via
        // the MCP/proxy path rather than directly from the RP2040 serial link.
        if bridge.has_explicit_task_state || bridge.last_thread_status.is_some() {
            if bridge.task_board.has_tasks() || bridge.last_thread_status.is_none() {
                bridge
                    .task_board
                    .rendered_slots(&bridge.task_device_id, bridge.task_slot_count)
            } else {
                bridge.fused_lcd.fused_array(&bridge.partition)
            }
        } else {
            connection_default_cards(
                bridge
                    .partition
                    .slots_for(crate::routing::AgentId::Codex)
                    .len(),
            )
        }
    } else if bridge.has_explicit_task_state || bridge.last_thread_status.is_some() {
        bridge.fused_lcd.fused_array(&bridge.partition)
    } else {
        connection_default_cards(bridge.task_slot_count)
    }
}

fn display_context_for_controller(
    bridge: &BridgeRuntime,
    context: &DisplayContext,
    device_id: &str,
) -> DisplayContext {
    let Some(slot) = bridge.task_board.selected_slot(device_id) else {
        return context.clone();
    };
    let Some(task) = context.task.as_deref() else {
        return context.clone();
    };
    let mut rendered = context.clone();
    rendered.task = Some(crate::tasks::display_task_title(slot, task));
    rendered
}

fn desired_context(bridge: &BridgeRuntime) -> Option<DisplayContext> {
    desired_context_for_device(bridge, &bridge.task_device_id)
}

/// Auto-derives a display context from thstatus entries and stores it as
/// `last_display_context` when no explicit MCP `set_display_context` has
/// been called. The thstatus entries from the RP2040 carry only slot
/// colors and enabled state — no model, effort, or usage — so those
/// fields are left null and shown as "—" on the strips. The status is
/// derived from the slot color, matching the HID palette.
pub(crate) fn auto_derive_display_context(bridge: &mut BridgeRuntime) {
    if bridge.has_explicit_display_context {
        return;
    }
    // Find the first enabled task across all devices.
    let mut best: Option<(String, usize, u32, String)> = None;
    for task in bridge.task_board.tasks() {
        if task.state == crate::tasks::TaskState::Completed {
            continue;
        }
        let Some(ref assignment) = bridge.task_board.assignment(&task.task_id) else {
            continue;
        };
        let color = task.color.unwrap_or(0x37474f);
        let slot = assignment.slot.slot;
        let device_id = assignment.slot.device_id.clone();
        // Prefer the first enabled slot on any device.
        if best.is_none() || slot < best.as_ref().unwrap().1 {
            best = Some((device_id, slot, color, task.state.as_str().to_owned()));
        }
    }
    let Some((device_id, slot, color, _state_str)) = best else {
        return;
    };
    let status = color_to_status(color);
    let context = DisplayContext {
        project: Some("Codex".to_owned()),
        task: Some(format!("Slot {}", slot + 1)),
        model: None,
        effort: None,
        status: Some(status.to_owned()),
        progress: None,
        task_id: Some(format!("codex-hid:{}", slot)),
        weekly_remaining: None,
        five_hour_remaining: None,
    };
    bridge.last_display_context = Some(context);
    let _ = bridge.task_board.select(&device_id, slot, now_ms());
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Maps a thstatus color to a status string, matching the HID palette.
fn color_to_status(color: u32) -> &'static str {
    match color {
        0x1565c0 => "working",
        0x6a1b9a => "thinking",
        0xef6c00 => "waiting",
        0xb71c1c => "error",
        0x2e7d32 => "done",
        0x0277bd => "ready",
        _ => "idle",
    }
}

/// Computes the display context for a specific controller device, looking
/// up the selected task on that device rather than the primary controller.
fn desired_context_for_device(bridge: &BridgeRuntime, device_id: &str) -> Option<DisplayContext> {
    bridge
        .last_display_context
        .clone()
        .or_else(|| {
            bridge
                .task_board
                .selected_display_context(device_id)
                .and_then(|value| DisplayContext::from_value(&value).ok())
        })
        .or_else(|| Some(connection_default_context()))
        .map(|context| display_context_for_controller(bridge, &context, device_id))
}

fn apply_controller_state(
    device: &mut dyn PhysicalController,
    cards: &[Value],
    context: Option<&DisplayContext>,
    rgb_config: Option<&Value>,
) -> Result<(), String> {
    device.apply_task_cards(cards)?;
    if let Some(context) = context {
        device.apply_display_context(context)?;
    }
    if let Some(config) = rgb_config {
        device.apply_rgb_config(config)?;
    }
    Ok(())
}

fn replay_primary_controller_state(bridge: &mut BridgeRuntime) -> Result<(), String> {
    let cards = desired_primary_cards(bridge);
    let context = desired_context(bridge);
    let rgb_config = bridge.last_rgb_config.clone();
    if let Some(device) = bridge.controller.as_mut() {
        apply_controller_state(
            device.as_mut(),
            &cards,
            context.as_ref(),
            rgb_config.as_ref(),
        )?;
    }
    Ok(())
}
pub(crate) fn refresh_task_board(bridge: &mut BridgeRuntime) -> Result<(), String> {
    if !bridge.task_mode {
        return Ok(());
    }
    let cards = desired_primary_cards(bridge);
    let context = desired_context(bridge);
    let rgb_config = bridge.last_rgb_config.clone();
    if let Some(device) = bridge.controller.as_mut() {
        if let Err(error) = apply_controller_state(
            device.as_mut(),
            &cards,
            context.as_ref(),
            rgb_config.as_ref(),
        ) {
            detach_controller(bridge, &error);
            return Err(error);
        }
    }
    let mut survivors = Vec::new();
    for (device_id, mut device, slots) in std::mem::take(&mut bridge.aux_controllers) {
        let cards = if bridge.has_explicit_task_state || bridge.last_thread_status.is_some() {
            if bridge.task_board.has_tasks() {
                bridge.task_board.rendered_slots(&device_id, slots)
            } else {
                bridge.fused_lcd.fused_array(&bridge.partition)
            }
        } else {
            connection_default_cards(slots)
        };
        let context = desired_context_for_device(bridge, &device_id);
        let result = apply_controller_state(
            device.as_mut(),
            &cards,
            context.as_ref(),
            rgb_config.as_ref(),
        );
        if let Err(error) = result {
            device.shutdown();
            eprintln!("device {device_id} render failed: {error}");
            bridge.task_board.set_device(&device_id, 0, false);
        } else {
            survivors.push((device_id, device, slots));
        }
    }
    bridge.aux_controllers = survivors;
    Ok(())
}

pub struct HealthCheck {
    pub next_probe_at: Instant,
    pub pending_deadline: Option<Instant>,
}

impl HealthCheck {
    fn new() -> Self {
        Self {
            next_probe_at: Instant::now() + HEALTH_CHECK_INTERVAL,
            pending_deadline: None,
        }
    }

    fn due_at(&self, now: Instant) -> bool {
        self.pending_deadline.is_none() && now >= self.next_probe_at
    }

    fn begin_at(&mut self, now: Instant) {
        self.pending_deadline = Some(now + HEALTH_CHECK_TIMEOUT);
    }

    fn observe_status_at(&mut self, now: Instant) {
        self.pending_deadline = None;
        self.next_probe_at = now + HEALTH_CHECK_INTERVAL;
    }

    fn timed_out_at(&self, now: Instant) -> bool {
        self.pending_deadline
            .is_some_and(|deadline| now >= deadline)
    }
}

fn open_serial_runtime(
    requested_port: &str,
) -> Result<(SerialRuntime, String, String, u16), String> {
    let port = serial::resolve_port(requested_port)?;
    let mut writer = serial::open(&port)?;
    let reader = writer
        .try_clone()
        .map_err(|error| format!("could not clone RP2040 port: {error}"))?;
    let (serial_tx, receiver) = mpsc::channel();
    let reader_thread = serial::start_reader(reader, serial_tx);
    let mut sequence = 1_u16;
    let firmware = wait_for_firmware(&receiver, &mut writer, &mut sequence)?;
    Ok((
        SerialRuntime {
            writer: Some(writer),
            receiver,
            reader_thread: Some(reader_thread),
        },
        firmware,
        port,
        sequence,
    ))
}

pub(crate) fn connect_controller(
    choice: ControllerKind,
    serial: Option<&str>,
) -> Result<Option<Box<dyn PhysicalController>>, String> {
    match choice {
        ControllerKind::None => Ok(None),
        ControllerKind::Ajazz => Ok(Some(Box::new(crate::ajazz::AjazzDevice::connect(serial)?))),
        ControllerKind::StreamDeckPlus
        | ControllerKind::StreamDeckPlusXl
        | ControllerKind::StreamDeckXl => Ok(Some(connect_streamdeck(choice, serial)?)),
        // The plugin controller is created by the daemon when a plugin
        // session arrives; it is never instantiated from the CLI.
        ControllerKind::StreamDeckPlugin => Err(
            "streamdeck-plugin controller is daemon-managed and not selectable from the CLI"
                .to_owned(),
        ),
    }
}

pub(crate) fn open_runtime(options: &Options) -> Result<BridgeRuntime, String> {
    let (serial, firmware, port, sequence) = if options.port == "none" {
        (
            None,
            String::from("standalone"),
            String::from("none"),
            1_u16,
        )
    } else {
        match open_serial_runtime(&options.port) {
            Ok((serial, firmware, port, sequence)) => (Some(serial), firmware, port, sequence),
            Err(error) if options.port == "auto" => {
                eprintln!("RP2040 unavailable at startup; continuing without serial: {error}");
                (None, String::from("disconnected"), String::from("auto"), 1_u16)
            }
            Err(error) => return Err(error),
        }
    };
    let controller = connect_controller(options.controller, options.controller_serial.as_deref())?;
    let task_device_id = controller
        .as_ref()
        .map(|device| device.device_id())
        .unwrap_or_else(|| options.controller.as_str().to_owned());
    let task_slot_count = controller
        .as_ref()
        .map(|device| {
            options
                .devices
                .first()
                .and_then(|spec| spec.task_slots)
                .unwrap_or_else(|| device.task_slot_count())
        })
        .unwrap_or(0);
    let mut task_board = crate::tasks::TaskBoard::new();
    if task_slot_count > 0 {
        task_board.set_device(task_device_id.clone(), task_slot_count, true);
    }
    let mut aux_controllers = Vec::new();
    for spec in options.devices.iter().skip(1) {
        let device = connect_controller(spec.kind, spec.serial.as_deref())?
            .ok_or_else(|| format!("device {} did not produce a controller", spec.kind.as_str()))?;
        let id = device.device_id();
        let slots = spec.task_slots.unwrap_or_else(|| device.task_slot_count());
        if slots > 0 {
            task_board.set_device(id.clone(), slots, true);
        }
        aux_controllers.push((id, device, slots));
    }
    let mut bridge = BridgeRuntime {
        serial,
        sequence,
        controller,
        aux_controllers: Vec::new(),
        controller_choice: options.controller,
        controller_serial: options.controller_serial.clone(),
        devices: options.devices.clone(),
        controller_retry_at: Instant::now() + CONTROLLER_RETRY_INITIAL_DELAY,
        controller_retry_delay: CONTROLLER_RETRY_INITIAL_DELAY,
        last_thread_status: None,
        has_explicit_task_state: false,
        last_rgb_config: None,
        last_display_context: None,
        has_explicit_display_context: false,
        firmware,
        port,
        codex_decoder: CodexDecoder::default(),
        radial_state: RadialState::default(),
        health: HealthCheck::new(),
        routing: crate::routing::EventRouting::new(),
        fused_lcd: crate::routing::FusedLcdState::new(),
        task_board,
        task_mode: false,
        task_device_id,
        task_slot_count,
        pending_task_events: Vec::new(),
        // In legacy/mcp modes, Codex is the sole agent and owns all 6 slots.
        // The daemon updates this dynamically based on active sessions.
        partition: crate::routing::Partition::compute(crate::routing::ActiveSet::from_single(
            crate::routing::AgentId::Codex,
        )),
    };
    replay_primary_controller_state(&mut bridge)?;
    let context = desired_context(&bridge);
    let rgb_config = bridge.last_rgb_config.clone();
    for (_, device, slots) in bridge.aux_controllers.iter_mut() {
        let cards = connection_default_cards(*slots);
        apply_controller_state(
            device.as_mut(),
            &cards,
            context.as_ref(),
            rgb_config.as_ref(),
        )?;
    }
    Ok(bridge)
}

pub(crate) fn replace_runtime(bridge: &mut BridgeRuntime, options: &Options) -> Result<(), String> {
    if let Some(mut serial) = bridge.serial.take() {
        drop(serial.writer.take());
        if let Some(reader_thread) = serial.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }
    if options.port == "none" {
        bridge.serial = None;
        bridge.firmware = String::from("standalone");
        bridge.port = String::from("none");
        bridge.sequence = 1;
        bridge.codex_decoder = CodexDecoder::default();
        bridge.health = HealthCheck::new();
        return Ok(());
    }
    let (serial, firmware, port, sequence) = open_serial_runtime(&options.port)?;
    bridge.serial = Some(serial);
    bridge.sequence = sequence;
    bridge.firmware = firmware;
    bridge.port = port;
    bridge.codex_decoder = CodexDecoder::default();
    bridge.health = HealthCheck::new();
    Ok(())
}

fn schedule_controller_retry(bridge: &mut BridgeRuntime) {
    if bridge.controller_choice.is_physical() {
        bridge.controller_retry_at = Instant::now() + bridge.controller_retry_delay;
        bridge.controller_retry_delay =
            (bridge.controller_retry_delay * 2).min(CONTROLLER_RETRY_MAX_DELAY);
    }
}

pub(crate) fn reconnect_controller_if_due(bridge: &mut BridgeRuntime) {
    if bridge.controller.is_some()
        || !bridge.controller_choice.is_physical()
        || Instant::now() < bridge.controller_retry_at
    {
        return;
    }
    match connect_controller(
        bridge.controller_choice,
        bridge.controller_serial.as_deref(),
    ) {
        Ok(Some(controller)) => {
            bridge.controller = Some(controller);
            if bridge.task_mode {
                bridge.task_board.set_device(
                    bridge.task_device_id.clone(),
                    bridge.task_slot_count,
                    true,
                );
            }
            if let Err(error) = replay_primary_controller_state(bridge) {
                eprintln!("controller state replay failed: {error}");
                detach_controller(bridge, &error);
                return;
            }
            if bridge.task_mode {
                let _ = refresh_task_board(bridge);
            }
            bridge.controller_retry_delay = CONTROLLER_RETRY_INITIAL_DELAY;
            bridge.controller_retry_at = Instant::now() + Duration::from_secs(5);
            eprintln!("controller reconnected");
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("controller reconnect pending: {error}");
            schedule_controller_retry(bridge);
        }
    }
}
pub(crate) fn bridge_status(bridge: &BridgeRuntime, mode: &str) -> Value {
    let controller = bridge.controller.as_ref();
    let mut devices = vec![json!({
        "id": bridge.task_device_id,
        "taskSlots": bridge.task_slot_count,
        "connected": controller.is_some(),
        "selectedTask": bridge.task_board.selected(&bridge.task_device_id)
    })];
    devices.extend(bridge.aux_controllers.iter().map(|(id, _, slots)| {
        json!({
            "id": id,
            "taskSlots": slots,
            "connected": true,
            "selectedTask": bridge.task_board.selected(id)
        })
    }));
    json!({
        "type": "bridge-ready",
        "version": 2,
        "firmware": bridge.firmware,
        "port": bridge.port,
        "rp2040": bridge.has_serial(),
        "ajazzConnected": controller.is_some_and(|device| device.kind() == ControllerKind::Ajazz),
        "controller": {
            "kind": bridge.controller_choice.as_str(),
            "connected": controller.is_some(),
            "model": controller.map(|device| device.model()),
            "serial": controller.and_then(|device| device.serial())
        },
        "displayContext": bridge.last_display_context.as_ref().map(DisplayContext::to_value),
        "agents": {
            "codex": {
                "events": bridge.routing.queue(crate::routing::AgentId::Codex).len(),
                "keys": bridge.partition.keys_for(crate::routing::AgentId::Codex),
                "slots": bridge.partition.slots_for(crate::routing::AgentId::Codex)
            },
            "zcode": {
                "events": bridge.routing.queue(crate::routing::AgentId::ZCode).len(),
                "keys": bridge.partition.keys_for(crate::routing::AgentId::ZCode),
                "slots": bridge.partition.slots_for(crate::routing::AgentId::ZCode)
            },
            "hermes": {
                "events": bridge.routing.queue(crate::routing::AgentId::Hermes).len(),
                "keys": bridge.partition.keys_for(crate::routing::AgentId::Hermes),
                "slots": bridge.partition.slots_for(crate::routing::AgentId::Hermes)
            }
        },
        "partition": {
            "owners": bridge.partition.owners_json()
        },
        "taskMode": bridge.task_mode,
        "devices": devices,
        "taskBoard": bridge.task_board.status_json(),
        "mode": mode
    })
}
pub(crate) fn detach_controller(bridge: &mut BridgeRuntime, error: &str) {
    if let Some(mut controller) = bridge.controller.take() {
        controller.shutdown();
        eprintln!("{} HID disconnected: {error}", controller.kind().as_str());
    }
    schedule_controller_retry(bridge);
}

/// Alias used by the daemon module.
pub(crate) fn detach_controller_for(bridge: &mut BridgeRuntime, error: &str) {
    detach_controller(bridge, error);
}

pub(crate) fn poll_controller(bridge: &mut BridgeRuntime, trace: bool) -> Result<(), String> {
    let result = bridge.controller.as_mut().map(|device| device.poll(25));
    match result {
        Some(Ok(events)) => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            for event in events {
                if bridge.task_mode {
                    if let crate::codex::PhysicalEvent::Button { index, pressed } = event {
                        if usize::from(index) < bridge.task_slot_count {
                            if let Some(selection) = bridge.task_board.select(
                                &bridge.task_device_id,
                                usize::from(index),
                                now_ms,
                            ) {
                                let owner = selection
                                    .get("owner_session")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0)
                                    as usize;
                                bridge.pending_task_events.push((owner, selection.clone()));
                                if let Some(legacy_key) =
                                    selection.get("legacy_key").and_then(Value::as_str)
                                {
                                    bridge.pending_task_events.push((owner, json!({"type":"key","key":legacy_key,"pressed":pressed,"ts":now_ms})));
                                    if owner == 0 && bridge.has_serial() {
                                        if let Ok(messages) = messages_for_synthetic_key(legacy_key)
                                        {
                                            if pressed {
                                                if let Some(message) = messages.first() {
                                                    let _ = bridge.send_codex(message);
                                                }
                                            } else if let Some(message) = messages.last() {
                                                let _ = bridge.send_codex(message);
                                            }
                                        }
                                    }
                                }
                                continue;
                            }
                        }
                    }
                }
                // Partition button events by agent based on the current
                // partition. Codex-owned buttons go to HID when the RP2040
                // is present, otherwise are buffered for the Codex MCP
                // session. Non-Codex buttons are always buffered for polling.
                if let crate::codex::PhysicalEvent::Button { index, pressed } = event {
                    if let Some(owner) = bridge.partition.owner_of(index) {
                        if owner != crate::routing::AgentId::Codex {
                            bridge
                                .routing
                                .route_button(index, pressed, now_ms, &bridge.partition);
                            if trace {
                                println!(
                                    "{}",
                                    json!({"type":"controller-event","controller":bridge.controller_choice.as_str(),"agent":owner.as_str(),"event":format!("{event:?}")})
                                );
                            }
                            continue;
                        }
                        // Codex button: buffer for the Codex MCP session in
                        // standalone mode (no HID path).
                        if !bridge.has_serial() {
                            bridge
                                .routing
                                .route_button(index, pressed, now_ms, &bridge.partition);
                        }
                    }
                }
                if let Some(message) = bridge.radial_state.event(event) {
                    if bridge.has_serial() {
                        bridge.send_codex(&message)?;
                    }
                    if trace {
                        println!(
                            "{}",
                            json!({"type":"controller-event","controller":bridge.controller_choice.as_str(),"event":format!("{event:?}")})
                        );
                    }
                }
            }
        }
        Some(Err(error)) => detach_controller(bridge, &error),
        None => std::thread::sleep(Duration::from_millis(25)),
    }
    let mut survivors = Vec::new();
    for (device_id, mut device, slots) in std::mem::take(&mut bridge.aux_controllers) {
        match device.poll(25) {
            Ok(events) => route_task_device_events(bridge, &device_id, slots, events),
            Err(error) => {
                eprintln!("device {device_id} poll failed: {error}");
                device.shutdown();
                continue;
            }
        }
        survivors.push((device_id, device, slots));
    }
    bridge.aux_controllers = survivors;
    Ok(())
}

fn route_task_device_events(
    bridge: &mut BridgeRuntime,
    device_id: &str,
    slot_count: usize,
    events: Vec<crate::codex::PhysicalEvent>,
) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    for event in events {
        // A task card gets first refusal while task mode is active. If the
        // slot has no task (or task mode is off), fall through to the normal
        // physical-event mapping so Agent/Action/Mic/Send/Dial keys work too.
        if bridge.task_mode {
            if let crate::codex::PhysicalEvent::Button { index, pressed } = event {
                if usize::from(index) < slot_count {
                    if let Some(selection) = bridge.task_board.select(
                        device_id,
                        usize::from(index),
                        now_ms,
                    ) {
                        let owner = selection
                            .get("owner_session")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                            as usize;
                        bridge.pending_task_events.push((owner, selection));
                        if let Some(legacy_key) = bridge
                            .task_board
                            .task_at(device_id, usize::from(index))
                            .and_then(|task| task.legacy_key.as_deref())
                        {
                            bridge.pending_task_events.push((
                                owner,
                                json!({"type":"key","key":legacy_key,"pressed":pressed,"ts":now_ms}),
                            ));
                            if owner == 0 && bridge.has_serial() {
                                if let Ok(messages) = messages_for_synthetic_key(legacy_key) {
                                    let message = if pressed {
                                        messages.first()
                                    } else {
                                        messages.last()
                                    };
                                    if let Some(message) = message {
                                        let _ = bridge.send_codex(message);
                                    }
                                }
                            }
                        }
                        continue;
                    }
                }
            }
        }

        // Plugin-backed controllers are auxiliary physical controllers. They
        // must share the same partition and HID routing as the primary device.
        if let crate::codex::PhysicalEvent::Button { index, pressed } = event {
            if let Some(owner) = bridge.partition.owner_of(index) {
                if owner != crate::routing::AgentId::Codex {
                    bridge
                        .routing
                        .route_button(index, pressed, now_ms, &bridge.partition);
                    continue;
                }
                if !bridge.has_serial() {
                    bridge
                        .routing
                        .route_button(index, pressed, now_ms, &bridge.partition);
                }
            }
        }
        if let Some(message) = bridge.radial_state.event(event) {
            if bridge.has_serial() {
                let _ = bridge.send_codex(&message);
            }
        }
    }
}
fn run_legacy(options: Options) -> Result<(), String> {
    let mut bridge = open_runtime(&options)?;
    println!("{}", bridge_status(&bridge, "legacy"));
    let deadline = options
        .listen_seconds
        .map(|seconds| Instant::now() + Duration::from_secs(seconds));
    let mut synthetic_emit_at = options
        .emit_key
        .as_ref()
        .map(|_| Instant::now() + Duration::from_secs(options.emit_after_seconds));
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(());
        }
        if synthetic_emit_at.is_some_and(|emit_at| Instant::now() >= emit_at) {
            let key = options
                .emit_key
                .as_deref()
                .expect("emit deadline requires a key");
            for message in messages_for_synthetic_key(key)? {
                bridge.send_codex(&message)?;
                std::thread::sleep(Duration::from_millis(50));
            }
            println!("{}", json!({"type":"synthetic-event","key":key}));
            synthetic_emit_at = None;
        }
        reconnect_controller_if_due(&mut bridge);
        let mut serial_taken = bridge.serial.take();
        if let Some(runtime) = serial_taken.as_mut() {
            let mut events = Vec::new();
            while let Ok(event) = runtime.receiver.try_recv() {
                events.push(event);
            }
            for event in events {
                match event {
                    SerialEvent::Frame(frame)
                        if frame.frame_type == FrameType::CodexOutputReport =>
                    {
                        println!("{}", codex_report_trace(&frame.payload, frame.sequence));
                        match bridge.codex_decoder.feed(&frame.payload) {
                            Ok(messages) => {
                                for message in messages {
                                    let writer = runtime.writer.as_mut();
                                    match process_codex_message(
                                        message,
                                        &mut bridge.controller,
                                        &mut bridge.last_thread_status,
                                        &mut bridge.last_rgb_config,
                                        &mut bridge.fused_lcd,
                                        &bridge.partition,
                                        writer,
                                        &mut bridge.sequence,
                                        true,
                                    ) {
                                        Ok(()) => {}
                                        Err(ProcessError::Controller(error)) => {
                                            detach_controller(&mut bridge, &error)
                                        }
                                        Err(ProcessError::Protocol(error)) => println!(
                                            "{}",
                                            json!({"type":"codex-parameter-error","error":error})
                                        ),
                                        Err(ProcessError::Serial(error)) => return Err(error),
                                    }
                                }
                            }
                            Err(error) => {
                                println!("{}", json!({"type":"codex-report-error","error":error}))
                            }
                        }
                    }
                    SerialEvent::Frame(frame) if frame.frame_type == FrameType::Log => println!(
                        "{}",
                        json!({"type":"firmware-log","message":String::from_utf8_lossy(&frame.payload)})
                    ),
                    SerialEvent::Frame(_) => {}
                    SerialEvent::ProtocolError(error) => {
                        println!("{}", json!({"type":"protocol-error","error":error}))
                    }
                    SerialEvent::Disconnected(error) => return Err(error),
                }
            }
        }
        bridge.serial = serial_taken;
        poll_controller(&mut bridge, true)?;
    }
}
fn tool_arguments(request: &Value) -> &Value {
    request
        .get("params")
        .and_then(|params| params.get("arguments"))
        .unwrap_or(&Value::Null)
}

/// Helper for the daemon: emit_key tool.
pub(crate) fn call_emit_key(bridge: &mut BridgeRuntime, arguments: &Value) -> Value {
    let Some(key) = arguments.get("key").and_then(Value::as_str) else {
        return mcp::tool_error("emit_key requires a key");
    };
    match messages_for_synthetic_key(key) {
        Ok(messages) => {
            let count = messages.len();
            let send_result = messages.iter().try_for_each(|message| {
                bridge.send_codex(message)?;
                std::thread::sleep(Duration::from_millis(50));
                Ok::<(), String>(())
            });
            send_result
                .map(|_| mcp::text_result(json!({"key": key, "messagesSent": count})))
                .unwrap_or_else(mcp::tool_error)
        }
        Err(error) => mcp::tool_error(error),
    }
}

/// Helper for the daemon: send_codex_message tool.
pub(crate) fn call_send_codex_message(bridge: &mut BridgeRuntime, arguments: &Value) -> Value {
    let Some(message) = arguments.get("message") else {
        return mcp::tool_error("send_codex_message requires message");
    };
    if !message.is_object() {
        return mcp::tool_error("message must be a JSON object");
    }
    match send_tool_message(bridge, message) {
        Ok(reports) => mcp::text_result(json!({"reportsSent": reports})),
        Err(error) => mcp::tool_error(error),
    }
}

/// Helper for the daemon: set_display_context tool.
pub(crate) fn call_set_display_context(bridge: &mut BridgeRuntime, arguments: &Value) -> Value {
    let context = match DisplayContext::from_value(arguments) {
        Ok(context) => context,
        Err(error) => return mcp::tool_error(error),
    };
    bridge.last_display_context = Some(context.clone());
    bridge.has_explicit_display_context = true;

    let render_context = display_context_for_controller(bridge, &context, &bridge.task_device_id);
    let primary_error = bridge
        .controller
        .as_mut()
        .and_then(|device| device.apply_display_context(&render_context).err());
    if let Some(error) = primary_error.as_deref() {
        detach_controller(bridge, error);
    }

    // Plugin-backed Stream Decks live in aux_controllers. Push context to them
    // immediately instead of waiting for an unrelated task-board refresh.
    let mut survivors = Vec::new();
    for (device_id, mut device, slots) in std::mem::take(&mut bridge.aux_controllers) {
        if let Err(error) = device.apply_display_context(&render_context) {
            device.shutdown();
            eprintln!("device {device_id} display context failed: {error}");
        } else {
            survivors.push((device_id, device, slots));
        }
    }
    bridge.aux_controllers = survivors;

    if let Some(error) = primary_error {
        mcp::tool_error(format!("Stream Deck dashboard disconnected: {error}"))
    } else {
        mcp::text_result(json!({
            "updated": true,
            "context": context.to_value()
        }))
    }
}

/// Helper for the daemon: set_rgb_config tool.
pub(crate) fn call_set_rgb_config(bridge: &mut BridgeRuntime, arguments: &Value) -> Value {
    let Some(config) = arguments.get("config") else {
        return mcp::tool_error("set_rgb_config requires config");
    };
    if !config.is_object() {
        return mcp::tool_error("config must be an object");
    }
    let message = json!({"m": "v.oai.rgbcfg", "p": config});
    match send_tool_message(bridge, &message) {
        Ok(reports) => mcp::text_result(json!({"reportsSent": reports})),
        Err(error) => mcp::tool_error(error),
    }
}

/// Helper for the daemon: device_status tool.
pub(crate) fn call_device_status(bridge: &mut BridgeRuntime) -> Value {
    let id = next_sequence(&mut bridge.sequence);
    let message = json!({"m": "device.status", "id": id});
    match send_tool_message(bridge, &message) {
        Ok(reports) => mcp::text_result(json!({"requestId": id, "reportsSent": reports})),
        Err(error) => mcp::tool_error(error),
    }
}

fn send_tool_message(bridge: &mut BridgeRuntime, message: &Value) -> Result<usize, String> {
    let reports = frame_json(message)?.len();
    bridge.send_codex(message)?;
    Ok(reports)
}

fn apply_thread_status_locally(
    controller: &mut Option<Box<dyn PhysicalController>>,
    fused_lcd: &mut crate::routing::FusedLcdState,
    partition: &crate::routing::Partition,
    agent: crate::routing::AgentId,
    status: &Value,
) -> Result<(), String> {
    let fused = fused_lcd.merge_from_agent(agent, status, partition)?;
    if let Some(device) = controller.as_mut() {
        device.apply_thread_status(&Value::Array(fused))?;
    }
    Ok(())
}

fn call_tool(request: &Value, bridge: &mut BridgeRuntime) -> Value {
    let params = request.get("params").unwrap_or(&Value::Null);
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return mcp::tool_error("tools/call requires params.name");
    };
    let arguments = tool_arguments(request);
    let result = match name {
        "bridge_status" => Ok(mcp::text_result(bridge_status(bridge, "mcp"))),
        "emit_key" => {
            let Some(key) = arguments.get("key").and_then(Value::as_str) else {
                return mcp::tool_error("emit_key requires a key");
            };
            match messages_for_synthetic_key(key) {
                Ok(messages) => {
                    let count = messages.len();
                    let send_result = messages.iter().try_for_each(|message| {
                        bridge.send_codex(message)?;
                        std::thread::sleep(Duration::from_millis(50));
                        Ok::<(), String>(())
                    });
                    send_result
                        .map(|_| mcp::text_result(json!({"key": key, "messagesSent": count})))
                }
                Err(error) => Err(error),
            }
        }
        "send_codex_message" => {
            let Some(message) = arguments.get("message") else {
                return mcp::tool_error("send_codex_message requires message");
            };
            if !message.is_object() {
                Err("message must be a JSON object".to_owned())
            } else {
                send_tool_message(bridge, message)
                    .map(|reports| mcp::text_result(json!({"reportsSent": reports})))
            }
        }
        "set_thread_status" => {
            let Some(status) = arguments.get("status") else {
                return mcp::tool_error("set_thread_status requires status");
            };
            if !status.is_array() {
                Err("status must be an array".to_owned())
            } else {
                bridge.last_thread_status = Some(status.clone());
                bridge.has_explicit_task_state = true;
                if let Err(error) = apply_thread_status_locally(
                    &mut bridge.controller,
                    &mut bridge.fused_lcd,
                    &bridge.partition,
                    crate::routing::AgentId::Codex,
                    status,
                ) {
                    detach_controller(bridge, &error);
                    return mcp::tool_error(format!("controller apply failed: {error}"));
                }
                let message = json!({"m": "v.oai.thstatus", "p": status});
                send_tool_message(bridge, &message).map(|reports| {
                    mcp::text_result(json!({
                        "updated": true,
                        "reportsSent": reports
                    }))
                })
            }
        }
        "set_display_context" => {
            let context = match DisplayContext::from_value(arguments) {
                Ok(context) => context,
                Err(error) => return mcp::tool_error(error),
            };
            bridge.last_display_context = Some(context.clone());
            bridge.has_explicit_display_context = true;
            let render_context = display_context_for_controller(bridge, &context, &bridge.task_device_id);
            let apply_result = bridge
                .controller
                .as_mut()
                .map(|device| device.apply_display_context(&render_context));
            if let Some(Err(error)) = apply_result {
                detach_controller(bridge, &error);
                Err(format!("Stream Deck dashboard disconnected: {error}"))
            } else {
                Ok(mcp::text_result(json!({
                    "updated": true,
                    "context": context.to_value()
                })))
            }
        }
        "set_rgb_config" => {
            let Some(config) = arguments.get("config") else {
                return mcp::tool_error("set_rgb_config requires config");
            };
            if !config.is_object() {
                Err("config must be an object".to_owned())
            } else {
                let message = json!({"m": "v.oai.rgbcfg", "p": config});
                send_tool_message(bridge, &message)
                    .map(|reports| mcp::text_result(json!({"reportsSent": reports})))
            }
        }
        "device_status" => {
            let id = next_sequence(&mut bridge.sequence);
            let message = json!({"m": "device.status", "id": id});
            send_tool_message(bridge, &message)
                .map(|reports| mcp::text_result(json!({"requestId": id, "reportsSent": reports})))
        }
        "poll_events" => {
            // In legacy --mcp mode the agent is always Codex.
            let timeout_ms = arguments
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(25_000);
            let has_events = !bridge
                .routing
                .queue(crate::routing::AgentId::Codex)
                .is_empty();
            if has_events {
                let events: Vec<Value> = bridge
                    .routing
                    .queue_mut(crate::routing::AgentId::Codex)
                    .drain()
                    .into_iter()
                    .map(|e| e.to_json())
                    .collect();
                Ok(mcp::text_result(json!({"events": events})))
            } else if timeout_ms > 0 {
                // Simple blocking wait in legacy mode.
                let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                while Instant::now() < deadline
                    && bridge
                        .routing
                        .queue(crate::routing::AgentId::Codex)
                        .is_empty()
                {
                    std::thread::sleep(Duration::from_millis(25));
                    crate::reconnect_controller_if_due(bridge);
                    let _ = crate::poll_controller(bridge, false);
                }
                let events: Vec<Value> = bridge
                    .routing
                    .queue_mut(crate::routing::AgentId::Codex)
                    .drain()
                    .into_iter()
                    .map(|e| e.to_json())
                    .collect();
                Ok(mcp::text_result(json!({"events": events})))
            } else {
                Ok(mcp::text_result(json!({"events": []})))
            }
        }
        _ => Err(format!("unknown MCP tool: {name}")),
    };
    result.unwrap_or_else(mcp::tool_error)
}

fn handle_mcp_request(request: Value, bridge: &mut BridgeRuntime) -> Result<(), String> {
    let id = request.get("id");
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        if id.is_some() {
            mcp::write_message(&mcp::error_response(
                id,
                -32600,
                "MCP request requires method",
            ))?;
        }
        return Ok(());
    };
    if method.starts_with("notifications/") {
        return Ok(());
    }
    let Some(id) = id else {
        return Ok(());
    };
    let result = match method {
        "initialize" => json!({
            "protocolVersion": mcp::PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "micro-emu-rp2040-bridge", "version": "0.1.0"},
            "instructions": format!(
                "Use bridge_status first. The colored numbered LCD cards and READY dashboard are standby indicators until you publish live state. Publish task cards with set_thread_status or publish_tasks. {} Hardware actions target the RP2040 on the configured serial port.",
                mcp::DISPLAY_CONTEXT_INSTRUCTIONS
            )
        }),
        "ping" => json!({}),
        "resources/list" => json!({"resources": []}),
        "resources/templates/list" => json!({"resourceTemplates": []}),
        "tools/list" => mcp::tools(),
        "tools/call" => call_tool(&request, bridge),
        _ => {
            mcp::write_message(&mcp::error_response(
                Some(id),
                -32601,
                format!("unknown MCP method: {method}"),
            ))?;
            return Ok(());
        }
    };
    mcp::write_message(&mcp::response(id, result))
}

fn reconnect_mcp(
    bridge: &mut BridgeRuntime,
    options: &Options,
    input: &Receiver<Result<Value, String>>,
) -> Result<bool, String> {
    let mut delay = SERIAL_RETRY_INITIAL_DELAY;
    loop {
        while let Ok(message) = input.try_recv() {
            match message {
                Ok(request) => {
                    if let Some(id) = request.get("id") {
                        mcp::write_message(&mcp::error_response(
                            Some(id),
                            -32001,
                            "RP2040 bridge is reconnecting",
                        ))?;
                    }
                }
                Err(error) if error == "MCP client closed stdin" => return Ok(false),
                Err(error) => return Err(error),
            }
        }

        match replace_runtime(bridge, options) {
            Ok(()) => {
                eprintln!("{}", bridge_status(bridge, "mcp-reconnected"));
                return Ok(true);
            }
            Err(error) => {
                eprintln!("RP2040 bridge reconnect pending: {error}");
                std::thread::sleep(delay);
                delay = (delay * 2).min(SERIAL_RETRY_MAX_DELAY);
            }
        }
    }
}

pub(crate) fn send_health_ping(bridge: &mut BridgeRuntime, now: Instant) -> Result<(), String> {
    if !bridge.health.due_at(now) {
        return Ok(());
    }
    let ping = Frame::new(
        FrameType::Ping,
        next_sequence(&mut bridge.sequence),
        Vec::new(),
    )
    .map_err(|error| error.to_string())?;
    let Some(writer) = bridge
        .serial
        .as_mut()
        .and_then(|runtime| runtime.writer.as_mut())
    else {
        return Ok(());
    };
    serial::write_frame(writer, &ping)?;
    bridge.health.begin_at(now);
    Ok(())
}

fn run_mcp(options: Options) -> Result<(), String> {
    let mut bridge = open_runtime(&options)?;
    eprintln!("{}", bridge_status(&bridge, "mcp"));
    let input = mcp::start_input_reader();
    loop {
        while let Ok(message) = input.try_recv() {
            match message {
                Ok(message) => handle_mcp_request(message, &mut bridge)?,
                Err(error) if error == "MCP client closed stdin" => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        let mut serial_disconnected = None;
        // Drain serial events by temporarily taking the serial runtime out
        // of the bridge, so we can mutably access other bridge fields while
        // processing them.
        let mut serial_taken = bridge.serial.take();
        if let Some(runtime) = serial_taken.as_mut() {
            let mut events = Vec::new();
            while let Ok(event) = runtime.receiver.try_recv() {
                events.push(event);
            }
            for event in events {
                match event {
                    SerialEvent::Frame(frame)
                        if frame.frame_type == FrameType::CodexOutputReport =>
                    {
                        match bridge.codex_decoder.feed(&frame.payload) {
                            Ok(messages) => {
                                for message in messages {
                                    let writer = runtime.writer.as_mut();
                                    match process_codex_message(
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
                                        Err(ProcessError::Controller(error)) => {
                                            detach_controller(&mut bridge, &error)
                                        }
                                        Err(ProcessError::Protocol(error)) => {
                                            eprintln!("MCP bridge Codex parameter error: {error}")
                                        }
                                        Err(ProcessError::Serial(error)) => {
                                            serial_disconnected = Some(error);
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(error) => eprintln!("MCP bridge Codex report error: {error}"),
                        }
                    }
                    SerialEvent::Frame(frame) if frame.frame_type == FrameType::Status => {
                        bridge.health.observe_status_at(Instant::now())
                    }
                    SerialEvent::Frame(frame) if frame.frame_type == FrameType::Log => {
                        eprintln!("RP2040: {}", String::from_utf8_lossy(&frame.payload))
                    }
                    SerialEvent::Frame(_) => {}
                    SerialEvent::ProtocolError(error) => {
                        eprintln!("bridge protocol error: {error}")
                    }
                    SerialEvent::Disconnected(error) => {
                        serial_disconnected = Some(error);
                        break;
                    }
                }
            }
        }
        bridge.serial = serial_taken;
        if let Some(error) = serial_disconnected {
            eprintln!("RP2040 bridge disconnected: {error}");
            if !reconnect_mcp(&mut bridge, &options, &input)? {
                return Ok(());
            }
            continue;
        }
        let now = Instant::now();
        if bridge.health.timed_out_at(now) {
            eprintln!("RP2040 bridge health check timed out; reconnecting");
            if !reconnect_mcp(&mut bridge, &options, &input)? {
                return Ok(());
            }
            continue;
        }
        if let Err(error) = send_health_ping(&mut bridge, now) {
            eprintln!("RP2040 bridge health check failed: {error}");
            if !reconnect_mcp(&mut bridge, &options, &input)? {
                return Ok(());
            }
            continue;
        }
        reconnect_controller_if_due(&mut bridge);
        poll_controller(&mut bridge, false)?;
    }
}
impl Drop for BridgeRuntime {
    fn drop(&mut self) {
        if let Some(controller) = self.controller.as_mut() {
            controller.shutdown();
        }
        for (_, controller, _) in &mut self.aux_controllers {
            controller.shutdown();
        }
    }
}
fn run() -> Result<(), String> {
    let options = parse_options()?;
    if options.mcp_proxy {
        let agent = options.agent.expect("--mcp-proxy requires --agent");
        let exe = env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "rp2040-bridge".to_owned());
        return crate::proxy::run_proxy(crate::proxy::ProxyOptions {
            connect: options.connect,
            agent,
            autostart: options.autostart,
            daemon_args: options.daemon_args,
            exe,
        });
    }
    if options.daemon {
        return crate::daemon::run_daemon(crate::daemon::DaemonOptions {
            bind: options.bind.clone(),
            bridge_options: options,
        });
    }
    let use_mcp = options.mcp || (!options.legacy && !io::stdin().is_terminal());
    if use_mcp {
        run_mcp(options)
    } else {
        run_legacy(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct RecordingController {
        context: Arc<Mutex<Option<DisplayContext>>>,
        rendered: Arc<Mutex<Option<Value>>>,
    }

    impl PhysicalController for RecordingController {
        fn kind(&self) -> ControllerKind {
            ControllerKind::StreamDeckPlus
        }

        fn model(&self) -> &'static str {
            "test"
        }

        fn serial(&self) -> Option<&str> {
            Some("test")
        }

        fn poll(&mut self, _timeout_ms: i32) -> Result<Vec<crate::codex::PhysicalEvent>, String> {
            Ok(Vec::new())
        }

        fn apply_thread_status(&mut self, parameters: &Value) -> Result<(), String> {
            *self.rendered.lock().expect("render lock") = Some(parameters.clone());
            Ok(())
        }

        fn apply_display_context(&mut self, context: &DisplayContext) -> Result<(), String> {
            *self.context.lock().expect("context lock") = Some(context.clone());
            Ok(())
        }

        fn apply_rgb_config(&mut self, _parameters: &Value) -> Result<(), String> {
            Ok(())
        }

        fn shutdown(&mut self) {}
    }

    fn test_bridge(rendered: Arc<Mutex<Option<Value>>>) -> BridgeRuntime {
        test_bridge_with_context(rendered, Arc::new(Mutex::new(None)))
    }

    fn test_bridge_with_context(
        rendered: Arc<Mutex<Option<Value>>>,
        context: Arc<Mutex<Option<DisplayContext>>>,
    ) -> BridgeRuntime {
        BridgeRuntime {
            serial: None,
            sequence: 1,
            controller: Some(Box::new(RecordingController { rendered, context })),
            aux_controllers: Vec::new(),
            controller_choice: ControllerKind::StreamDeckPlus,
            controller_serial: Some("test".to_owned()),
            devices: Vec::new(),
            controller_retry_at: Instant::now(),
            controller_retry_delay: CONTROLLER_RETRY_INITIAL_DELAY,
            last_thread_status: None,
            has_explicit_task_state: false,
            last_rgb_config: None,
            last_display_context: None,
            has_explicit_display_context: false,
            firmware: "test".to_owned(),
            port: "none".to_owned(),
            codex_decoder: CodexDecoder::default(),
            radial_state: RadialState::default(),
            health: HealthCheck::new(),
            routing: crate::routing::EventRouting::new(),
            fused_lcd: crate::routing::FusedLcdState::new(),
            partition: crate::routing::Partition::compute(crate::routing::ActiveSet::from_single(
                crate::routing::AgentId::Codex,
            )),
            task_board: crate::tasks::TaskBoard::new(),
            task_mode: false,
            task_device_id: "streamdeck-plus:test".to_owned(),
            task_slot_count: 8,
            pending_task_events: Vec::new(),
        }
    }

    #[test]
    fn connection_defaults_are_visible_cards() {
        let cards = connection_default_cards(8);
        assert_eq!(cards.len(), 8);
        assert!(cards.iter().all(|card| card["e"] == 1));
        assert!(cards.iter().all(|card| card["agent"] == "codex"));
        assert!(cards.iter().all(|card| card["b"] == 0.70));
    }

    #[test]
    fn replay_defaults_after_controller_reconnect() {
        let rendered = Arc::new(Mutex::new(None));
        let mut bridge = test_bridge(Arc::clone(&rendered));
        replay_primary_controller_state(&mut bridge).expect("default replay");
        let payload = rendered
            .lock()
            .expect("render lock")
            .clone()
            .expect("payload");
        assert_eq!(payload.as_array().expect("cards").len(), 8);
        assert_eq!(payload[0]["e"], 1);

        let reconnected = Arc::new(Mutex::new(None));
        bridge.controller = Some(Box::new(RecordingController {
            rendered: Arc::clone(&reconnected),
            context: Arc::new(Mutex::new(None)),
        }));
        replay_primary_controller_state(&mut bridge).expect("reconnect replay");
        let payload = reconnected
            .lock()
            .expect("render lock")
            .clone()
            .expect("payload");
        assert_eq!(payload[0]["agent"], "codex");
        assert_eq!(payload[7]["e"], 1);
    }

    #[test]
    fn selected_task_context_is_prefixed_on_replay_and_direct_updates() {
        let rendered = Arc::new(Mutex::new(None));
        let contexts = Arc::new(Mutex::new(None));
        let mut bridge = test_bridge_with_context(Arc::clone(&rendered), Arc::clone(&contexts));
        let no_selection = test_bridge(Arc::new(Mutex::new(None)));
        let raw_context = DisplayContext {
            task: Some("Raw task".to_owned()),
            ..DisplayContext::default()
        };
        assert_eq!(
            display_context_for_controller(&no_selection, &raw_context, &no_selection.task_device_id)
                .task
                .as_deref(),
            Some("Raw task")
        );
        bridge.task_mode = true;
        bridge.has_explicit_task_state = true;
        bridge
            .task_board
            .set_device(bridge.task_device_id.clone(), 6, true);
        bridge
            .task_board
            .publish_tasks(
                1,
                crate::routing::AgentId::Codex,
                &json!({"tasks": [{
                    "task_id": "build",
                    "title": "Build bridge",
                    "state": "running"
                }]}),
                1,
            )
            .expect("publish task");
        bridge
            .task_board
            .select(&bridge.task_device_id, 0, 2)
            .expect("select task");

        replay_primary_controller_state(&mut bridge).expect("initial replay");
        assert_eq!(
            contexts
                .lock()
                .expect("context lock")
                .as_ref()
                .and_then(|context| context.task.as_deref()),
            Some("1 — Build bridge")
        );

        let reconnected_context = Arc::new(Mutex::new(None));
        bridge.controller = Some(Box::new(RecordingController {
            rendered: Arc::clone(&rendered),
            context: Arc::clone(&reconnected_context),
        }));
        replay_primary_controller_state(&mut bridge).expect("reconnect replay");
        assert_eq!(
            reconnected_context
                .lock()
                .expect("context lock")
                .as_ref()
                .and_then(|context| context.task.as_deref()),
            Some("1 — Build bridge")
        );

        call_set_display_context(&mut bridge, &json!({"task": "Codex refresh"}));
        assert_eq!(
            reconnected_context
                .lock()
                .expect("context lock")
                .as_ref()
                .and_then(|context| context.task.as_deref()),
            Some("1 — Codex refresh")
        );
        assert_eq!(
            bridge
                .last_display_context
                .as_ref()
                .and_then(|context| context.task.as_deref()),
            Some("Codex refresh")
        );
    }

    #[test]
    fn task_refresh_does_not_replace_mcp_display_context() {
        let rendered = Arc::new(Mutex::new(None));
        let contexts = Arc::new(Mutex::new(None));
        let mut bridge = test_bridge_with_context(rendered, Arc::clone(&contexts));
        bridge.task_mode = true;
        bridge.has_explicit_task_state = true;
        bridge
            .task_board
            .set_device(bridge.task_device_id.clone(), 6, true);

        call_set_display_context(
            &mut bridge,
            &json!({
                "task": "Live context",
                "model": "gpt-live",
                "effort": "medium",
                "weekly_remaining": 41,
                "five_hour_remaining": 87
            }),
        );
        bridge
            .task_board
            .publish_tasks(
                1,
                crate::routing::AgentId::Codex,
                &json!({"tasks": [{
                    "task_id": "stale",
                    "title": "Stale card",
                    "state": "running",
                    "context": {
                        "model": "gpt-5.2",
                        "effort": "high",
                        "weekly_remaining": 62,
                        "five_hour_remaining": 62
                    }
                }]}),
                1,
            )
            .expect("publish task");
        bridge
            .task_board
            .select(&bridge.task_device_id, 0, 2)
            .expect("select task");

        refresh_task_board(&mut bridge).expect("refresh task board");
        let context = contexts
            .lock()
            .expect("context lock")
            .clone()
            .expect("context");
        assert_eq!(context.model.as_deref(), Some("gpt-live"));
        assert_eq!(context.effort.as_deref(), Some("medium"));
        assert_eq!(context.weekly_remaining, Some(41));
        assert_eq!(context.five_hour_remaining, Some(87));
    }

    #[test]
    fn display_context_is_broadcast_to_aux_controllers() {
        let primary_context = Arc::new(Mutex::new(None));
        let aux_context = Arc::new(Mutex::new(None));
        let mut bridge =
            test_bridge_with_context(Arc::new(Mutex::new(None)), Arc::clone(&primary_context));
        bridge.aux_controllers.push((
            "streamdeck-plugin:test".to_owned(),
            Box::new(RecordingController {
                rendered: Arc::new(Mutex::new(None)),
                context: Arc::clone(&aux_context),
            }),
            4,
        ));

        call_set_display_context(
            &mut bridge,
            &json!({"project": "micro-emu", "task": "Live task", "progress": 42}),
        );

        assert_eq!(
            primary_context
                .lock()
                .expect("primary context")
                .as_ref()
                .and_then(|context| context.task.as_deref()),
            Some("Live task")
        );
        assert_eq!(
            aux_context
                .lock()
                .expect("aux context")
                .as_ref()
                .and_then(|context| context.task.as_deref()),
            Some("Live task")
        );
        assert_eq!(bridge.aux_controllers.len(), 1);
    }

    #[test]
    fn explicit_empty_status_clears_connection_defaults() {
        let rendered = Arc::new(Mutex::new(None));
        let mut bridge = test_bridge(Arc::clone(&rendered));
        bridge.has_explicit_task_state = true;
        bridge
            .fused_lcd
            .merge_from_agent(
                crate::routing::AgentId::Codex,
                &json!([]),
                &bridge.partition,
            )
            .expect("empty status merge");
        replay_primary_controller_state(&mut bridge).expect("empty status replay");
        let payload = rendered
            .lock()
            .expect("render lock")
            .clone()
            .expect("payload");
        assert!(
            payload
                .as_array()
                .expect("cards")
                .iter()
                .all(|card| card["e"] == 0)
        );
    }
    #[test]
    fn partition_refresh_does_not_clear_connection_defaults() {
        let rendered = Arc::new(Mutex::new(None));
        let mut bridge = test_bridge(Arc::clone(&rendered));
        bridge.task_mode = true;
        refresh_task_board(&mut bridge).expect("default task replay");
        let payload = rendered
            .lock()
            .expect("render lock")
            .clone()
            .expect("payload");
        let cards = payload.as_array().expect("cards");
        assert!(cards[..6].iter().all(|card| card["e"] == 1));
        assert!(cards[6..].iter().all(|card| card["e"] == 0));
    }
    #[test]
    fn standalone_status_render_updates_local_controller() {
        let rendered = Arc::new(Mutex::new(None));
        let mut controller: Option<Box<dyn PhysicalController>> =
            Some(Box::new(RecordingController {
                rendered: Arc::clone(&rendered),
                context: Arc::new(Mutex::new(None)),
            }));
        let mut fused_lcd = crate::routing::FusedLcdState::new();
        let partition = crate::routing::Partition::compute(crate::routing::ActiveSet::from_single(
            crate::routing::AgentId::Codex,
        ));
        let status = json!([{"c": "#112233", "b": 1.0}]);

        apply_thread_status_locally(
            &mut controller,
            &mut fused_lcd,
            &partition,
            crate::routing::AgentId::Codex,
            &status,
        )
        .expect("local status render");

        let payload = rendered
            .lock()
            .expect("render lock")
            .clone()
            .expect("rendered payload");
        assert_eq!(payload[0]["id"], 0);
        assert_eq!(payload[0]["i"], 0);
        assert_eq!(payload[0]["agent"], "codex");
        assert_eq!(payload[0]["c"], "#112233");
    }

    #[test]
    fn rpc_ack_preserves_vendor_call_id() {
        let message = json!({"method": "v.oai.rgbcfg", "params": {}, "id": 893});
        assert_eq!(
            rpc_ack_response(&message),
            Some(json!({"result": true, "id": 893}))
        );
    }

    #[test]
    fn notifications_do_not_receive_an_ack() {
        let message = json!({"m": "v.oai.rgbcfg", "p": {}});
        assert_eq!(rpc_ack_response(&message), None);
    }

    #[test]
    fn health_check_requires_a_status_response() {
        let now = Instant::now();
        let mut health = HealthCheck {
            next_probe_at: now,
            pending_deadline: None,
        };
        assert!(health.due_at(now));

        health.begin_at(now);
        assert!(!health.due_at(now));
        assert!(health.timed_out_at(now + HEALTH_CHECK_TIMEOUT));

        health.observe_status_at(now);
        assert!(!health.timed_out_at(now + HEALTH_CHECK_TIMEOUT));
        assert!(!health.due_at(now));
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
