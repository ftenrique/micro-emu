//! Virtual `PhysicalController` backed by a Stream Deck plugin session.
//!
//! The daemon accepts a plugin hello (`role: "controller"`) and registers the
//! session as a controller. Inbound plugin lines (key/dial events) are fed to
//! `poll`; outbound render state (thread status, task cards, display context,
//! RGB config) is forwarded to the plugin as JSON lines so it can update the
//! Stream Deck keys/dials.

use crate::codex::{CatalogAction, PhysicalEvent};
use crate::controller::{ControllerKind, DisplayContext, PhysicalController};
use serde_json::{Value, json};
use std::sync::mpsc::{self, Receiver, Sender};

/// Maximum task slots a plugin may report. Matches the `DeviceSpec` cap.
const MAX_PLUGIN_TASK_SLOTS: usize = 64;

/// Outbound line sender: the daemon writes JSON values here and the session
/// writer thread delivers them to the plugin socket.
pub type PluginWriter = Sender<Value>;

/// Inbound event channel: the session reader thread pushes parsed plugin
/// messages here; `poll` drains them.
pub type PluginEventReceiver = Receiver<Value>;

/// A virtual controller backed by a Stream Deck plugin session.
pub struct PluginController {
    /// Daemon-assigned device id (e.g. `streamdeck-plugin:instance-123`).
    device_id: String,
    /// Instance id from the plugin hello, used as the serial equivalent.
    instance_id: String,
    /// Inbound plugin messages (events + capacity updates).
    events: PluginEventReceiver,
    /// Outbound render/state lines to the plugin.
    writer: PluginWriter,
    /// Current task-slot capacity reported by the plugin.
    task_slots: usize,
    /// Set true when the session socket has closed.
    disconnected: bool,
    /// Last successfully transmitted state for each render channel. The daemon
    /// refresh loop is intentionally frequent, so suppressing identical
    /// payloads here prevents socket and Stream Deck image-update floods.
    last_task_cards: Option<Value>,
    last_thread_status: Option<Value>,
    last_display_context: Option<Value>,
    last_rgb_config: Option<Value>,
}

impl PluginController {
    /// Creates a new plugin controller bound to the given session channels.
    pub fn new(
        instance_id: String,
        task_slots: usize,
        events: PluginEventReceiver,
        writer: PluginWriter,
    ) -> Self {
        let device_id = format!("streamdeck-plugin:{instance_id}");
        Self {
            device_id,
            instance_id,
            events,
            writer,
            task_slots: task_slots.min(MAX_PLUGIN_TASK_SLOTS),
            disconnected: false,
            last_task_cards: None,
            last_thread_status: None,
            last_display_context: None,
            last_rgb_config: None,
        }
    }

    /// Sends a JSON line to the plugin. Returns an error if the channel is
    /// closed, which the daemon treats as a detach condition.
    fn send(&mut self, message: Value) -> Result<(), String> {
        if self.disconnected {
            return Err("plugin controller disconnected".to_owned());
        }
        self.writer
            .send(message)
            .map_err(|_| "plugin session writer closed".to_owned())
    }

    /// Parses an inbound plugin message into a `PhysicalEvent` (if it is an
    /// event) or applies a capacity update (if it is a capacity message).
    fn handle_inbound(&mut self, message: &Value) -> Option<PhysicalEvent> {
        let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "event" => parse_event(message),
            "capacity" => {
                if let Some(slots) = message.get("taskSlots").and_then(Value::as_u64) {
                    self.task_slots = (slots as usize).min(MAX_PLUGIN_TASK_SLOTS);
                }
                None
            }
            _ => None,
        }
    }
}

fn event_task_id(message: &Value) -> Option<Option<String>> {
    match message.get("task_id") {
        None => Some(None),
        Some(Value::String(task_id))
            if !task_id.is_empty() && task_id.chars().count() <= 160 =>
        {
            Some(Some(task_id.clone()))
        }
        _ => None,
    }
}

fn parse_event(message: &Value) -> Option<PhysicalEvent> {
    let event_kind = message.get("kind").and_then(Value::as_str)?;
    match event_kind {
        "button" => {
            let index = message.get("index").and_then(Value::as_u64)? as u8;
            let pressed = message
                .get("pressed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Some(PhysicalEvent::Button { index, pressed })
        }
        "task-button" => {
            let index = message.get("index").and_then(Value::as_u64)? as u8;
            let pressed = message
                .get("pressed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Some(PhysicalEvent::TaskButton {
                index,
                pressed,
                task_id: event_task_id(message)?,
            })
        }
        "task-toggle" => {
            let index = message.get("index").and_then(Value::as_u64)? as u8;
            Some(PhysicalEvent::TaskToggle {
                index,
                task_id: event_task_id(message)?,
            })
        }
        "task-action" => {
            let index = message.get("index").and_then(Value::as_u64)? as u8;
            let gesture = match message.get("gesture").and_then(Value::as_str)? {
                "short" => 0,
                "long" => 1,
                _ => return None,
            };
            Some(PhysicalEvent::TaskAction {
                index,
                gesture,
                task_id: event_task_id(message)?,
            })
        }
        "micro-key" => {
            let key = message.get("key").and_then(Value::as_str)?;
            let index = match key {
                "AG00" | "AG01" | "AG02" | "AG03" | "AG04" | "AG05" => key.as_bytes()[3] - b'0',
                "ACT06" | "ACT07" | "ACT08" => key.as_bytes()[4] - b'0',
                _ => return None,
            };
            let pressed = message
                .get("pressed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Some(PhysicalEvent::MicroButton { index, pressed })
        }
        "catalog-action" => {
            let action = CatalogAction::parse(message.get("action").and_then(Value::as_str)?)?;
            Some(PhysicalEvent::CatalogAction { action })
        }
        "model-cycle" => Some(PhysicalEvent::ModelCycle),
        "encoder-turn" => {
            let index = message.get("index").and_then(Value::as_u64)? as u8;
            let delta = message
                .get("delta")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(-127, 127) as i8;
            Some(PhysicalEvent::EncoderTurn { index, delta })
        }
        "encoder-button" => {
            let index = message.get("index").and_then(Value::as_u64)? as u8;
            let pressed = message
                .get("pressed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Some(PhysicalEvent::EncoderButton { index, pressed })
        }
        _ => None,
    }
}

impl PhysicalController for PluginController {
    fn kind(&self) -> ControllerKind {
        ControllerKind::StreamDeckPlugin
    }

    fn model(&self) -> &'static str {
        "streamdeck-plugin"
    }

    fn serial(&self) -> Option<&str> {
        Some(&self.instance_id)
    }

    fn poll(&mut self, _timeout_ms: i32) -> Result<Vec<PhysicalEvent>, String> {
        let mut events = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(message) => {
                    if let Some(event) = self.handle_inbound(&message) {
                        events.push(event);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    return Err("plugin session closed".to_owned());
                }
            }
        }
        Ok(events)
    }

    fn apply_task_cards(&mut self, cards: &[Value]) -> Result<(), String> {
        let cards = Value::Array(cards.to_vec());
        if self.last_task_cards.as_ref() == Some(&cards) {
            return Ok(());
        }
        self.send(json!({
            "type": "render",
            "render": "taskCards",
            "taskCards": &cards,
        }))?;
        self.last_task_cards = Some(cards);
        Ok(())
    }

    fn apply_thread_status(&mut self, parameters: &Value) -> Result<(), String> {
        if self.last_thread_status.as_ref() == Some(parameters) {
            return Ok(());
        }
        self.send(json!({
            "type": "render",
            "render": "threadStatus",
            "threadStatus": parameters,
        }))?;
        self.last_thread_status = Some(parameters.clone());
        Ok(())
    }

    fn apply_rgb_config(&mut self, parameters: &Value) -> Result<(), String> {
        if self.last_rgb_config.as_ref() == Some(parameters) {
            return Ok(());
        }
        self.send(json!({
            "type": "render",
            "render": "rgbConfig",
            "rgbConfig": parameters,
        }))?;
        self.last_rgb_config = Some(parameters.clone());
        Ok(())
    }

    fn apply_display_context(&mut self, context: &DisplayContext) -> Result<(), String> {
        let context = context.to_value();
        if self.last_display_context.as_ref() == Some(&context) {
            return Ok(());
        }
        self.send(json!({
            "type": "render",
            "render": "displayContext",
            "displayContext": &context,
        }))?;
        self.last_display_context = Some(context);
        Ok(())
    }
    fn task_slot_count(&self) -> usize {
        self.task_slots
    }

    fn device_id(&self) -> String {
        self.device_id.clone()
    }

    fn shutdown(&mut self) {
        self.disconnected = true;
        // A final goodbye helps the plugin tear down cleanly; ignore failure.
        let _ = self.writer.send(json!({"type":"goodbye"}));
    }
}

/// Parses a plugin hello line into the controller identity. Returns
/// `Some((instance_id, task_slots))` when the line is a controller hello,
/// otherwise `None` (the line is treated as an agent hello or MCP request).
pub fn parse_controller_hello(line: &str) -> Option<(String, usize)> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("bridge").and_then(Value::as_str) != Some("hello") {
        return None;
    }
    if value.get("role").and_then(Value::as_str) != Some("controller") {
        return None;
    }
    if value.get("controller").and_then(Value::as_str) != Some("streamdeck-plugin") {
        return None;
    }
    let instance_id = value
        .get("instance_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.chars().count() <= 160)
        .map(str::to_owned)?;
    let task_slots = value.get("taskSlots").and_then(Value::as_u64).unwrap_or(0) as usize;
    Some((instance_id, task_slots.min(MAX_PLUGIN_TASK_SLOTS)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn make_controller(slots: usize) -> (PluginController, Sender<Value>, Receiver<Value>) {
        let (events_tx, events_rx) = mpsc::channel::<Value>();
        let (writer_tx, writer_rx) = mpsc::channel::<Value>();
        let controller =
            PluginController::new("instance-test".to_owned(), slots, events_rx, writer_tx);
        (controller, events_tx, writer_rx)
    }

    #[test]
    fn parses_controller_hello_with_task_slots() {
        let line = r#"{"bridge":"hello","version":1,"role":"controller","controller":"streamdeck-plugin","instance_id":"abc","taskSlots":6}"#;
        let (instance, slots) = parse_controller_hello(line).expect("valid controller hello");
        assert_eq!(instance, "abc");
        assert_eq!(slots, 6);
    }

    #[test]
    fn rejects_agent_hello_as_controller_hello() {
        let line = r#"{"bridge":"hello","version":1,"agent":"codex","instance_id":"x"}"#;
        assert!(parse_controller_hello(line).is_none());
    }

    #[test]
    fn rejects_non_hello_line() {
        let line = r#"{"jsonrpc":"2.0","method":"initialize","id":1}"#;
        assert!(parse_controller_hello(line).is_none());
    }

    #[test]
    fn poll_translates_button_events() {
        let (mut controller, events_tx, _writer_rx) = make_controller(4);
        events_tx
            .send(json!({"type":"event","kind":"button","index":3,"pressed":true}))
            .unwrap();
        events_tx
            .send(json!({"type":"event","kind":"button","index":3,"pressed":false}))
            .unwrap();
        let events = controller.poll(0).expect("poll");
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            PhysicalEvent::Button {
                index: 3,
                pressed: true
            }
        );
        assert_eq!(
            events[1],
            PhysicalEvent::Button {
                index: 3,
                pressed: false
            }
        );
    }

    #[test]
    fn poll_keeps_task_micro_and_catalog_events_distinct() {
        let (mut controller, events_tx, _writer_rx) = make_controller(8);
        events_tx
            .send(json!({"type":"event","kind":"task-button","index":0,"pressed":true,"task_id":"thread-finished"}))
            .unwrap();
        events_tx
            .send(json!({"type":"event","kind":"task-toggle","index":1,"task_id":"thread-running"}))
            .unwrap();
        events_tx
            .send(json!({"type":"event","kind":"micro-key","key":"AG00","pressed":true}))
            .unwrap();
        events_tx
            .send(json!({"type":"event","kind":"catalog-action","action":"task.retry"}))
            .unwrap();
        events_tx
            .send(json!({"type":"event","kind":"model-cycle"}))
            .unwrap();

        assert_eq!(
            controller.poll(0).expect("poll"),
            vec![
                PhysicalEvent::TaskButton {
                    index: 0,
                    pressed: true,
                    task_id: Some("thread-finished".to_owned()),
                },
                PhysicalEvent::TaskToggle {
                    index: 1,
                    task_id: Some("thread-running".to_owned()),
                },
                PhysicalEvent::MicroButton {
                    index: 0,
                    pressed: true,
                },
                PhysicalEvent::CatalogAction {
                    action: CatalogAction::TaskRetry,
                },
                PhysicalEvent::ModelCycle,
            ]
        );
    }

    #[test]
    fn rejects_unknown_micro_keys_and_catalog_actions() {
        assert!(
            parse_event(&json!({
                "type":"event", "kind":"micro-key", "key":"AG99", "pressed":true
            }))
            .is_none()
        );
        assert!(
            parse_event(&json!({
                "type":"event", "kind":"catalog-action", "action":"task.delete-forever"
            }))
            .is_none()
        );
        assert!(
            parse_event(&json!({
                "type":"event", "kind":"task-button", "index":0, "task_id":""
            }))
            .is_none()
        );
    }

    #[test]
    fn poll_translates_encoder_events() {
        let (mut controller, events_tx, _writer_rx) = make_controller(4);
        events_tx
            .send(json!({"type":"event","kind":"encoder-turn","index":0,"delta":1}))
            .unwrap();
        events_tx
            .send(json!({"type":"event","kind":"encoder-button","index":1,"pressed":true}))
            .unwrap();
        let events = controller.poll(0).expect("poll");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], PhysicalEvent::EncoderTurn { index: 0, delta: 1 });
        assert_eq!(
            events[1],
            PhysicalEvent::EncoderButton {
                index: 1,
                pressed: true
            }
        );
    }

    #[test]
    fn capacity_updates_task_slot_count() {
        let (mut controller, events_tx, _writer_rx) = make_controller(2);
        events_tx
            .send(json!({"type":"capacity","taskSlots":8}))
            .unwrap();
        let _ = controller.poll(0).expect("poll");
        assert_eq!(controller.task_slot_count(), 8);
    }

    #[test]
    fn capacity_is_clamped_to_max() {
        let (mut controller, events_tx, _writer_rx) = make_controller(2);
        events_tx
            .send(json!({"type":"capacity","taskSlots":999}))
            .unwrap();
        let _ = controller.poll(0).expect("poll");
        assert_eq!(controller.task_slot_count(), MAX_PLUGIN_TASK_SLOTS);
    }

    #[test]
    fn apply_thread_status_sends_render_line() {
        let (mut controller, _events_tx, writer_rx) = make_controller(4);
        let status = json!([{"id":0,"e":1,"c":65280}]);
        controller.apply_thread_status(&status).expect("apply");
        let line = writer_rx.recv().expect("render line");
        assert_eq!(line["type"], "render");
        assert_eq!(line["render"], "threadStatus");
        assert_eq!(line["threadStatus"], status);
    }

    #[test]
    fn apply_display_context_sends_render_line() {
        let (mut controller, _events_tx, writer_rx) = make_controller(4);
        let context = DisplayContext {
            project: Some("micro-emu".to_owned()),
            task: Some("plugin".to_owned()),
            model: Some("gpt-5".to_owned()),
            effort: None,
            status: Some("working".to_owned()),
            progress: Some(50),
            task_id: None,
            weekly_remaining: None,
            five_hour_remaining: None,
            weekly_reset_at: None,
            five_hour_reset_at: None,
            wait_reason: None,
            prompt: None,
            interaction_id: None,
            short_action: None,
            long_action: None,
            pending_wait_count: None,
        };
        controller.apply_display_context(&context).expect("apply");
        let line = writer_rx.recv().expect("render line");
        assert_eq!(line["render"], "displayContext");
        assert_eq!(line["displayContext"]["project"], "micro-emu");
        assert_eq!(line["displayContext"]["progress"], 50);
    }

    #[test]
    fn duplicate_render_payloads_are_suppressed() {
        let (mut controller, _events_tx, writer_rx) = make_controller(4);
        let cards = vec![json!({"id": 0, "e": 1, "status": "running"})];
        let status = json!([{"id": 0, "e": 1, "c": 65280}]);
        let config = json!({"brightness": 80});
        let context = DisplayContext {
            project: Some("micro-emu".to_owned()),
            task: Some("dedupe".to_owned()),
            model: None,
            effort: None,
            status: Some("working".to_owned()),
            progress: None,
            task_id: None,
            weekly_remaining: None,
            five_hour_remaining: None,
            weekly_reset_at: None,
            five_hour_reset_at: None,
            wait_reason: None,
            prompt: None,
            interaction_id: None,
            short_action: None,
            long_action: None,
            pending_wait_count: None,
        };

        controller.apply_task_cards(&cards).unwrap();
        controller.apply_task_cards(&cards).unwrap();
        controller.apply_thread_status(&status).unwrap();
        controller.apply_thread_status(&status).unwrap();
        controller.apply_rgb_config(&config).unwrap();
        controller.apply_rgb_config(&config).unwrap();
        controller.apply_display_context(&context).unwrap();
        controller.apply_display_context(&context).unwrap();

        let rendered: Vec<Value> = writer_rx.try_iter().collect();
        assert_eq!(rendered.len(), 4);
        assert_eq!(rendered[0]["render"], "taskCards");
        assert_eq!(rendered[1]["render"], "threadStatus");
        assert_eq!(rendered[2]["render"], "rgbConfig");
        assert_eq!(rendered[3]["render"], "displayContext");
    }
    #[test]
    fn poll_returns_error_when_session_closes() {
        let (mut controller, events_tx, _writer_rx) = make_controller(4);
        drop(events_tx);
        let result = controller.poll(0);
        assert!(result.is_err());
        assert!(controller.disconnected);
    }

    #[test]
    fn shutdown_sends_goodbye_and_marks_disconnected() {
        let (mut controller, _events_tx, writer_rx) = make_controller(4);
        controller.shutdown();
        let line = writer_rx.recv().expect("goodbye");
        assert_eq!(line["type"], "goodbye");
        assert!(controller.disconnected);
    }

    #[test]
    fn device_id_uses_instance_id() {
        let (controller, _events_tx, _writer_rx) = make_controller(4);
        assert_eq!(controller.device_id(), "streamdeck-plugin:instance-test");
        assert_eq!(controller.serial(), Some("instance-test"));
    }
}
