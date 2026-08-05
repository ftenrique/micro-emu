mod ajazz;
mod codex;
mod controller;
mod daemon;
mod mcp;
mod proxy;
mod routing;
mod serial;
mod streamdeck;
mod wire;

use crate::codex::{CodexDecoder, RadialState, frame_json, messages_for_synthetic_key};
use crate::controller::{ControllerKind, DisplayContext, PhysicalController};
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

const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Options {
    pub port: String,
    pub controller: ControllerKind,
    pub controller_serial: Option<String>,
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
                    .ok_or_else(|| "--agent requires codex or hermes".to_owned())?;
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
                    "rp2040-bridge --port COMx|auto|none [--controller ajazz|streamdeck-plus|streamdeck-xl|none] [--controller-serial SERIAL] [--no-ajazz] [--listen 0..3600] [--emit AG00] [--emit-after 0..3600] [--mcp|--legacy|--daemon [--bind 127.0.0.1:48360] | --mcp-proxy --agent codex|hermes [--connect 127.0.0.1:48360] [--autostart] [--daemon-args \"...\"]]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    let mode_count = [mcp, legacy, daemon, mcp_proxy].iter().filter(|&&f| f).count();
    if mode_count > 1 {
        return Err("--mcp, --legacy, --daemon, and --mcp-proxy are mutually exclusive".to_owned());
    }
    if no_ajazz && controller.is_some() {
        return Err("--no-ajazz cannot be combined with --controller".to_owned());
    }
    if mcp_proxy && agent.is_none() {
        return Err("--mcp-proxy requires --agent codex|hermes".to_owned());
    }
    let controller = controller.unwrap_or(if no_ajazz {
        ControllerKind::None
    } else {
        ControllerKind::Ajazz
    });
    if controller_serial.is_some() && !controller.is_physical() {
        return Err("--controller-serial requires a physical --controller".to_owned());
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
                // ChatGPT (HID) owns slots 0-2 (Codex). Merge only that
                // range into the fused state and apply the full fused array.
                let fused = match fused_lcd.merge_from_agent(
                    crate::routing::AgentId::Codex,
                    parameters,
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
    pub controller_choice: ControllerKind,
    pub controller_serial: Option<String>,
    pub controller_retry_at: Instant,
    pub controller_retry_delay: Duration,
    pub last_thread_status: Option<Value>,
    pub last_rgb_config: Option<Value>,
    pub last_display_context: Option<DisplayContext>,
    pub firmware: String,
    pub port: String,
    pub codex_decoder: CodexDecoder,
    pub radial_state: RadialState,
    pub health: HealthCheck,
    pub routing: crate::routing::EventRouting,
    pub fused_lcd: crate::routing::FusedLcdState,
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
        ControllerKind::Ajazz => Ok(Some(Box::new(crate::ajazz::AjazzDevice::connect()?))),
        ControllerKind::StreamDeckPlus | ControllerKind::StreamDeckXl => {
            Ok(Some(connect_streamdeck(choice, serial)?))
        }
    }
}

pub(crate) fn open_runtime(options: &Options) -> Result<BridgeRuntime, String> {
    let (serial, firmware, port, sequence) = if options.port == "none" {
        (None, String::from("standalone"), String::from("none"), 1_u16)
    } else {
        let (serial, firmware, port, sequence) = open_serial_runtime(&options.port)?;
        (Some(serial), firmware, port, sequence)
    };
    let controller = connect_controller(options.controller, options.controller_serial.as_deref())?;
    Ok(BridgeRuntime {
        serial,
        sequence,
        controller,
        controller_choice: options.controller,
        controller_serial: options.controller_serial.clone(),
        controller_retry_at: Instant::now() + Duration::from_secs(1),
        controller_retry_delay: Duration::from_millis(500),
        last_thread_status: None,
        last_rgb_config: None,
        last_display_context: None,
        firmware,
        port,
        codex_decoder: CodexDecoder::default(),
        radial_state: RadialState::default(),
        health: HealthCheck::new(),
        routing: crate::routing::EventRouting::new(),
        fused_lcd: crate::routing::FusedLcdState::new(),
    })
}

pub(crate) fn replace_runtime(bridge: &mut BridgeRuntime, options: &Options) -> Result<(), String> {
    if let Some(serial) = bridge.serial.as_mut() {
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
            (bridge.controller_retry_delay * 2).min(Duration::from_secs(5));
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
        Ok(Some(mut controller)) => {
            let fused = bridge.fused_lcd.fused_array();
            let fused_value = Value::Array(fused);
            if let Err(error) = controller.apply_thread_status(&fused_value) {
                eprintln!("controller state replay failed: {error}");
                schedule_controller_retry(bridge);
                return;
            }
            if let Some(value) = bridge.last_rgb_config.as_ref() {
                if let Err(error) = controller.apply_rgb_config(value) {
                    eprintln!("controller RGB replay failed: {error}");
                    schedule_controller_retry(bridge);
                    return;
                }
            }
            if let Some(context) = bridge.last_display_context.as_ref() {
                if let Err(error) = controller.apply_display_context(context) {
                    eprintln!("controller display context replay failed: {error}");
                    schedule_controller_retry(bridge);
                    return;
                }
            }
            bridge.controller = Some(controller);
            bridge.controller_retry_delay = Duration::from_millis(500);
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
    json!({
        "type": "bridge-ready",
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
            "codex": {"events": bridge.routing.queue(crate::routing::AgentId::Codex).len()},
            "hermes": {"events": bridge.routing.queue(crate::routing::AgentId::Hermes).len()}
        },
        "partition": {
            "codex": {"keys": ["AG00", "AG01", "AG02"], "slots": [0, 1, 2]},
            "hermes": {"keys": ["AG03", "AG04", "AG05"], "slots": [3, 4, 5]}
        },
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
                // Partition button events by agent. Buttons 0-2 belong to
                // Codex (sent to HID when the RP2040 is present, otherwise
                // buffered for the Codex MCP session). Buttons 3-5 belong to
                // Hermes and are always buffered for polling.
                if let crate::codex::PhysicalEvent::Button { index, pressed } = event {
                    if let Some(owner) = crate::routing::button_owner(index) {
                        if owner == crate::routing::AgentId::Hermes {
                            bridge.routing.route_button(index, pressed, now_ms);
                            if trace {
                                println!(
                                    "{}",
                                    json!({"type":"controller-event","controller":bridge.controller_choice.as_str(),"agent":"hermes","event":format!("{event:?}")})
                                );
                            }
                            continue;
                        }
                        // Codex button: buffer for the Codex MCP session in
                        // standalone mode (no HID path).
                        if !bridge.has_serial() {
                            bridge.routing.route_button(index, pressed, now_ms);
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
    Ok(())
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
                    SerialEvent::Frame(frame) if frame.frame_type == FrameType::CodexOutputReport => {
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
    let apply_result = bridge
        .controller
        .as_mut()
        .map(|device| device.apply_display_context(&context));
    if let Some(Err(error)) = apply_result {
        detach_controller(bridge, &error);
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
                let message = json!({"m": "v.oai.thstatus", "p": status});
                send_tool_message(bridge, &message)
                    .map(|reports| mcp::text_result(json!({"reportsSent": reports})))
            }
        }
        "set_display_context" => {
            let context = match DisplayContext::from_value(arguments) {
                Ok(context) => context,
                Err(error) => return mcp::tool_error(error),
            };
            bridge.last_display_context = Some(context.clone());
            let apply_result = bridge
                .controller
                .as_mut()
                .map(|device| device.apply_display_context(&context));
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
            let has_events = !bridge.routing.queue(crate::routing::AgentId::Codex).is_empty();
            if has_events {
                let events: Vec<Value> = bridge
                    .routing
                    .queue_mut(crate::routing::AgentId::Codex)
                    .drain()
                    .into_iter()
                    .map(|e| json!({"key": e.key, "pressed": e.pressed, "ts": e.timestamp_ms}))
                    .collect();
                Ok(mcp::text_result(json!({"events": events})))
            } else if timeout_ms > 0 {
                // Simple blocking wait in legacy mode.
                let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                while Instant::now() < deadline
                    && bridge.routing.queue(crate::routing::AgentId::Codex).is_empty()
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
                    .map(|e| json!({"key": e.key, "pressed": e.pressed, "ts": e.timestamp_ms}))
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
            "instructions": "Use bridge_status first. Hardware actions target the RP2040 on the configured serial port."
        }),
        "ping" => json!({}),
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
    let mut delay = Duration::from_millis(500);
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
                delay = (delay * 2).min(Duration::from_secs(5));
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
                    SerialEvent::Frame(frame) if frame.frame_type == FrameType::CodexOutputReport => {
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
                    SerialEvent::ProtocolError(error) => eprintln!("bridge protocol error: {error}"),
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
