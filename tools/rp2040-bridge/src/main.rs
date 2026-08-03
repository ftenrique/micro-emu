mod ajazz;
mod codex;
mod mcp;
mod serial;
mod wire;

use crate::ajazz::AjazzDevice;
use crate::codex::{frame_json, messages_for_synthetic_key, CodexDecoder, RadialState};
use crate::serial::SerialEvent;
use crate::wire::{Frame, FrameType};
use serde_json::{json, Value};
use std::env;
use std::fs::File;
use std::io::{self, IsTerminal};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

struct Options {
    port: String,
    no_ajazz: bool,
    listen_seconds: Option<u64>,
    emit_key: Option<String>,
    emit_after_seconds: u64,
    mcp: bool,
    legacy: bool,
}

fn parse_options() -> Result<Options, String> {
    let mut port = None;
    let mut no_ajazz = false;
    let mut listen_seconds = None;
    let mut emit_key = None;
    let mut emit_after_seconds = 3;
    let mut mcp = false;
    let mut legacy = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--" => {}
            "--port" => {
                port = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--port requires COMx or auto".to_owned())?,
                );
            }
            "--no-ajazz" => no_ajazz = true,
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
            "--help" | "-h" => {
                println!(
                    "rp2040-bridge --port COMx|auto [--no-ajazz] [--listen 0..3600 (0=unlimited)] \
                     [--emit AG00] [--emit-after 0..3600] [--mcp|--legacy]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if mcp && legacy {
        return Err("--mcp and --legacy cannot be used together".to_owned());
    }
    if let Some(key) = &emit_key {
        messages_for_synthetic_key(key)?;
    }
    Ok(Options {
        port: port.ok_or_else(|| "--port COMx is required".to_owned())?,
        no_ajazz,
        listen_seconds,
        emit_key,
        emit_after_seconds,
        mcp,
        legacy,
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

fn process_codex_message(
    message: Value,
    ajazz: &mut Option<AjazzDevice>,
    writer: &mut File,
    sequence: &mut u16,
    trace: bool,
) -> Result<(), String> {
    let method = codex::method(&message).map(str::to_owned);
    match method.as_deref() {
        Some("device.status") => {}
        Some("v.oai.thstatus") => {
            let parameters = message.get("p").or_else(|| message.get("params"));
            if let Some(device) = ajazz {
                if let Some(parameters) = parameters {
                    device.apply_thread_status(parameters)?;
                }
            }
        }
        Some("v.oai.rgbcfg") => {
            let parameters = message.get("p").or_else(|| message.get("params"));
            if let Some(device) = ajazz {
                if let Some(parameters) = parameters {
                    device.apply_rgb_config(parameters)?;
                }
            }
        }
        _ => {}
    }
    if let Some(response) = rpc_ack_response(&message) {
        send_codex_message(writer, sequence, &response)?;
        if trace {
            println!(
                "{}",
                json!({
                    "type": "codex-response",
                    "method": method,
                    "id": message.get("id"),
                    "result": true
                })
            );
        }
    }
    if trace {
        println!(
            "{}",
            json!({
                "type": "codex-message",
                "method": method,
                "id": message.get("id")
            })
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

struct SerialRuntime {
    writer: Option<File>,
    receiver: Receiver<SerialEvent>,
    reader_thread: Option<JoinHandle<()>>,
}

struct BridgeRuntime {
    serial: SerialRuntime,
    sequence: u16,
    ajazz: Option<AjazzDevice>,
    firmware: String,
    port: String,
    codex_decoder: CodexDecoder,
    radial_state: RadialState,
    health: HealthCheck,
}

struct HealthCheck {
    next_probe_at: Instant,
    pending_deadline: Option<Instant>,
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

fn open_runtime(options: &Options) -> Result<BridgeRuntime, String> {
    let (serial, firmware, port, sequence) = open_serial_runtime(&options.port)?;
    let ajazz = if options.no_ajazz {
        None
    } else {
        Some(AjazzDevice::connect()?)
    };
    Ok(BridgeRuntime {
        serial,
        sequence,
        ajazz,
        firmware,
        port,
        codex_decoder: CodexDecoder::default(),
        radial_state: RadialState::default(),
        health: HealthCheck::new(),
    })
}

fn replace_runtime(bridge: &mut BridgeRuntime, options: &Options) -> Result<(), String> {
    drop(bridge.serial.writer.take());
    if let Some(reader_thread) = bridge.serial.reader_thread.take() {
        let _ = reader_thread.join();
    }
    // A system suspend can invalidate the AJAZZ HID handle without producing
    // a CDC read error. Drop it before reopening both device paths.
    bridge.ajazz.take();

    let (serial, firmware, port, sequence) = open_serial_runtime(&options.port)?;
    let ajazz = if options.no_ajazz {
        None
    } else {
        Some(AjazzDevice::connect()?)
    };
    bridge.serial = serial;
    bridge.ajazz = ajazz;
    bridge.sequence = sequence;
    bridge.firmware = firmware;
    bridge.port = port;
    bridge.codex_decoder = CodexDecoder::default();
    bridge.health = HealthCheck::new();
    Ok(())
}

fn bridge_status(bridge: &BridgeRuntime, mode: &str) -> Value {
    json!({
        "type": "bridge-ready",
        "firmware": bridge.firmware,
        "port": bridge.port,
        "ajazzConnected": bridge.ajazz.is_some(),
        "mode": mode
    })
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
                send_codex_message(
                    bridge
                        .serial
                        .writer
                        .as_mut()
                        .expect("serial writer is present"),
                    &mut bridge.sequence,
                    &message,
                )?;
                std::thread::sleep(Duration::from_millis(50));
            }
            println!("{}", json!({"type": "synthetic-event", "key": key}));
            synthetic_emit_at = None;
        }
        while let Ok(event) = bridge.serial.receiver.try_recv() {
            match event {
                SerialEvent::Frame(frame) if frame.frame_type == FrameType::CodexOutputReport => {
                    println!("{}", codex_report_trace(&frame.payload, frame.sequence));
                    match bridge.codex_decoder.feed(&frame.payload) {
                        Ok(messages) => {
                            for message in messages {
                                process_codex_message(
                                    message,
                                    &mut bridge.ajazz,
                                    bridge
                                        .serial
                                        .writer
                                        .as_mut()
                                        .expect("serial writer is present"),
                                    &mut bridge.sequence,
                                    true,
                                )?;
                            }
                        }
                        Err(error) => {
                            println!("{}", json!({"type": "codex-report-error", "error": error}))
                        }
                    }
                }
                SerialEvent::Frame(frame) if frame.frame_type == FrameType::Log => println!(
                    "{}",
                    json!({
                        "type": "firmware-log",
                        "message": String::from_utf8_lossy(&frame.payload)
                    })
                ),
                SerialEvent::Frame(_) => {}
                SerialEvent::ProtocolError(error) => {
                    println!("{}", json!({"type": "protocol-error", "error": error}));
                }
                SerialEvent::Disconnected(error) => return Err(error),
            }
        }
        if let Some(device) = &mut bridge.ajazz {
            for event in device.poll(25)? {
                if let Some(message) = bridge.radial_state.event(event) {
                    send_codex_message(
                        bridge
                            .serial
                            .writer
                            .as_mut()
                            .expect("serial writer is present"),
                        &mut bridge.sequence,
                        &message,
                    )?;
                    println!(
                        "{}",
                        json!({"type": "ajazz-event", "event": format!("{event:?}")})
                    );
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn tool_arguments(request: &Value) -> &Value {
    request
        .get("params")
        .and_then(|params| params.get("arguments"))
        .unwrap_or(&Value::Null)
}

fn send_tool_message(bridge: &mut BridgeRuntime, message: &Value) -> Result<usize, String> {
    let reports = frame_json(message)?.len();
    send_codex_message(
        bridge
            .serial
            .writer
            .as_mut()
            .expect("serial writer is present"),
        &mut bridge.sequence,
        message,
    )?;
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
                        send_codex_message(
                            bridge
                                .serial
                                .writer
                                .as_mut()
                                .expect("serial writer is present"),
                            &mut bridge.sequence,
                            message,
                        )?;
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

fn send_health_ping(bridge: &mut BridgeRuntime, now: Instant) -> Result<(), String> {
    if !bridge.health.due_at(now) {
        return Ok(());
    }
    let ping = Frame::new(
        FrameType::Ping,
        next_sequence(&mut bridge.sequence),
        Vec::new(),
    )
    .map_err(|error| error.to_string())?;
    serial::write_frame(
        bridge
            .serial
            .writer
            .as_mut()
            .expect("serial writer is present"),
        &ping,
    )?;
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

        let mut disconnected = None;
        while let Ok(event) = bridge.serial.receiver.try_recv() {
            match event {
                SerialEvent::Frame(frame) if frame.frame_type == FrameType::CodexOutputReport => {
                    match bridge.codex_decoder.feed(&frame.payload) {
                        Ok(messages) => {
                            for message in messages {
                                if let Err(error) = process_codex_message(
                                    message,
                                    &mut bridge.ajazz,
                                    bridge
                                        .serial
                                        .writer
                                        .as_mut()
                                        .expect("serial writer is present"),
                                    &mut bridge.sequence,
                                    false,
                                ) {
                                    disconnected =
                                        Some(format!("device processing failed: {error}"));
                                    break;
                                }
                            }
                        }
                        Err(error) => eprintln!("MCP bridge Codex report error: {error}"),
                    }
                }
                SerialEvent::Frame(frame) if frame.frame_type == FrameType::Status => {
                    bridge.health.observe_status_at(Instant::now());
                }
                SerialEvent::Frame(frame) if frame.frame_type == FrameType::Log => {
                    eprintln!("RP2040: {}", String::from_utf8_lossy(&frame.payload));
                }
                SerialEvent::Frame(_) => {}
                SerialEvent::ProtocolError(error) => eprintln!("bridge protocol error: {error}"),
                SerialEvent::Disconnected(error) => {
                    disconnected = Some(error);
                    break;
                }
            }
            if disconnected.is_some() {
                break;
            }
        }
        if let Some(error) = disconnected {
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

        let ajazz_poll = if let Some(device) = &mut bridge.ajazz {
            Some(device.poll(25))
        } else {
            None
        };
        match ajazz_poll {
            Some(Ok(events)) => {
                for event in events {
                    if let Some(message) = bridge.radial_state.event(event) {
                        send_codex_message(
                            bridge
                                .serial
                                .writer
                                .as_mut()
                                .expect("serial writer is present"),
                            &mut bridge.sequence,
                            &message,
                        )?;
                    }
                }
            }
            Some(Err(error)) => {
                eprintln!("AJAZZ HID disconnected: {error}");
                if !reconnect_mcp(&mut bridge, &options, &input)? {
                    return Ok(());
                }
                continue;
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
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
