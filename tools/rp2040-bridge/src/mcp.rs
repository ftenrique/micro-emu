use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn start_input_reader() -> Receiver<Result<Value, String>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("mcp-stdin".to_owned())
        .spawn(move || read_input(sender))
        .expect("MCP stdin reader thread should start");
    receiver
}

fn read_input(sender: Sender<Result<Value, String>>) {
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => {
                let parsed = serde_json::from_str(&line)
                    .map_err(|error| format!("invalid MCP JSON: {error}"));
                if sender.send(parsed).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(format!("MCP stdin read failed: {error}")));
                return;
            }
        }
    }
    let _ = sender.send(Err("MCP client closed stdin".to_owned()));
}

pub fn write_message(message: &Value) -> Result<(), String> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, message).map_err(|error| error.to_string())?;
    lock.write_all(b"\n").map_err(|error| error.to_string())?;
    lock.flush().map_err(|error| error.to_string())
}

pub fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn error_response(id: Option<&Value>, code: i64, message: impl Into<String>) -> Value {
    let id = id.cloned().unwrap_or(Value::Null);
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()}
    })
}

pub fn tools() -> Value {
    json!({
        "tools": [
            {
                "name": "bridge_status",
                "description": "Return the RP2040 bridge firmware, serial port and AJAZZ connection state.",
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
            },
            {
                "name": "emit_key",
                "description": "Emit a synthetic Codex Micro key press/release through the physical bridge.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string", "enum": ["AG00", "AG01", "AG02", "AG03", "AG04", "AG05", "ACT06", "ACT07", "ACT08", "ENC_CW", "ENC_CC", "ENC_CLK"]}
                    },
                    "required": ["key"],
                    "additionalProperties": false
                }
            },
            {
                "name": "send_codex_message",
                "description": "Send one Codex Micro JSON message to the RP2040 device.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"message": {"type": "object"}},
                    "required": ["message"],
                    "additionalProperties": false
                }
            },
            {
                "name": "set_thread_status",
                "description": "Update the six AJAZZ LCD status slots using v.oai.thstatus.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"status": {"type": "array", "items": {"type": "object"}}},
                    "required": ["status"],
                    "additionalProperties": false
                }
            },
            {
                "name": "set_rgb_config",
                "description": "Send the v.oai.rgbcfg configuration to the bridge.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"config": {"type": "object"}},
                    "required": ["config"],
                    "additionalProperties": false
                }
            },
            {
                "name": "device_status",
                "description": "Request device.status from the RP2040 firmware; the response is consumed by the bridge.",
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
            }
        ]
    })
}

pub fn text_result(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({"content": [{"type": "text", "text": text}], "structuredContent": value})
}

pub fn tool_error(message: impl Into<String>) -> Value {
    let message = message.into();
    json!({"isError": true, "content": [{"type": "text", "text": message}]})
}

