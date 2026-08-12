use crate::routing::AgentId;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const DISPLAY_CONTEXT_INSTRUCTIONS: &str = "The RP2040 v.oai.thstatus path updates task cards only; it does not update the dashboard model, effort, weekly_remaining, or five_hour_remaining fields. Call set_display_context with the current live metadata at task start and again whenever any display-context value changes. Standby or test values on the display are not live context.";

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
                "description": "Publish live Codex task/status state to the assigned LCD slots using v.oai.thstatus. The colored numbered cards shown before this call are standby indicators, not task data; call this whenever task status changes. This updates task cards only: call set_display_context separately for live model, effort, and usage metadata. Use bridge_status to see which slots you own.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"status": {"type": "array", "items": {"type": "object"}}},
                    "required": ["status"],
                    "additionalProperties": false
                }
            },
            {
                "name": "publish_tasks",
                "description": "Publish the complete live task-card snapshot for this MCP session. This replaces the standby LCD indicators; call it at task start and whenever the task list or progress changes. Task-card updates do not change display-context model, effort, or usage fields; publish those separately with set_display_context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tasks": {"type": "array", "items": {"type": "object"}}
                    },
                    "required": ["tasks"],
                    "additionalProperties": false
                }
            },
            {
                "name": "set_display_context",
                "description": "Publish live project/task/model/effort/status/progress and remaining weekly/five-hour usage metadata to the connected device displays. Call this at task start and whenever any display-context value changes; v.oai.thstatus and task-card tools do not update model, effort, or usage. A horizontal Stream Deck + swipe alternates the context and usage screens.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": {"type": ["string", "null"]},
                        "task": {"type": ["string", "null"]},
                        "model": {"type": ["string", "null"]},
                        "effort": {"type": ["string", "null"]},
                        "status": {"type": ["string", "null"]},
                        "progress": {"type": ["integer", "null"], "minimum": 0, "maximum": 100},
                        "task_id": {"type": ["string", "null"]},
                        "weekly_remaining": {"type": ["integer", "null"], "minimum": 0, "maximum": 100},
                        "five_hour_remaining": {"type": ["integer", "null"], "minimum": 0, "maximum": 100},
                    },
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
            },
            poll_events_tool()
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

/// Returns the tool list filtered for the given agent. `None` means the
/// agent has not been identified yet (e.g. direct `--mcp` STDIO mode) and
/// all tools are exposed for backward compatibility.
pub fn tools_for(agent: Option<AgentId>) -> Value {
    let all = tools()["tools"].as_array().unwrap_or(&Vec::new()).clone();
    let filtered: Vec<Value> = all
        .into_iter()
        .filter(|tool| {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
            tool_available(name, agent)
        })
        .collect();
    json!({"tools": filtered})
}

pub fn daemon_tools_for(agent: Option<AgentId>) -> Value {
    let mut result = tools_for(agent);
    if let Some(tools) = result.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|tool| tool.get("name").and_then(Value::as_str) != Some("set_rgb_config"));
    }
    result
}
/// Returns true if the given tool is available for the agent.
pub fn tool_available(name: &str, agent: Option<AgentId>) -> bool {
    match agent {
        Some(AgentId::Hermes) => matches!(
            name,
            "bridge_status"
                | "poll_events"
                | "set_thread_status"
                | "publish_tasks"
                | "set_rgb_config"
                | "set_display_context"
        ),
        Some(AgentId::ZCode) => matches!(
            name,
            "bridge_status"
                | "poll_events"
                | "set_thread_status"
                | "publish_tasks"
                | "set_rgb_config"
                | "set_display_context"
        ),
        // Codex or unknown (legacy --mcp): all tools.
        _ => true,
    }
}

/// The `poll_events` tool definition.
pub fn poll_events_tool() -> Value {
    json!({
        "name": "poll_events",
        "description": "Drain buffered physical controller events for the calling agent. With timeout_ms > 0, waits up to that many milliseconds for events to arrive (long-poll). Returns an array of events; key events have shape {type:\"key\", key, pressed, ts} and partition change events have shape {type:\"partition\", keys, slots, agents, ts}.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "timeout_ms": {"type": "integer", "minimum": 0, "maximum": 25000, "default": 0}
            },
            "additionalProperties": false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_guidance_requires_live_display_context_updates() {
        assert!(DISPLAY_CONTEXT_INSTRUCTIONS.contains("Call set_display_context"));
        assert!(
            DISPLAY_CONTEXT_INSTRUCTIONS.contains("whenever any display-context value changes")
        );

        let tools = tools();
        let definitions = tools["tools"].as_array().expect("tool definitions");
        let context_tool = definitions
            .iter()
            .find(|tool| tool["name"] == "set_display_context")
            .expect("set_display_context tool");
        let description = context_tool["description"].as_str().expect("description");
        assert!(description.contains("task-card tools do not update model, effort, or usage"));
    }

    #[test]
    fn hermes_has_agent_neutral_context_but_not_codex_transport_tools() {
        assert!(tool_available("bridge_status", Some(AgentId::Hermes)));
        assert!(tool_available("publish_tasks", Some(AgentId::Hermes)));
        assert!(tool_available("set_display_context", Some(AgentId::Hermes)));
        assert!(!tool_available("send_codex_message", Some(AgentId::Hermes)));
        assert!(!tool_available("device_status", Some(AgentId::Hermes)));
    }

    #[test]
    fn codex_tool_surface_is_unchanged() {
        for name in [
            "emit_key",
            "send_codex_message",
            "set_display_context",
            "device_status",
        ] {
            assert!(tool_available(name, Some(AgentId::Codex)), "missing {name}");
        }
    }
}
