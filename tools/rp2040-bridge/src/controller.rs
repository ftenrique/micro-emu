use crate::codex::PhysicalEvent;
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayContext {
    pub project: Option<String>,
    pub task: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub status: Option<String>,
    pub progress: Option<u8>,
    pub task_id: Option<String>,
    pub weekly_remaining: Option<u8>,
    pub five_hour_remaining: Option<u8>,
    pub wait_reason: Option<String>,
    pub prompt: Option<String>,
    pub interaction_id: Option<String>,
    pub short_action: Option<String>,
    pub long_action: Option<String>,
    pub pending_wait_count: Option<u8>,
}

impl DisplayContext {
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "display context must be an object".to_owned())?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "project"
                    | "task"
                    | "model"
                    | "effort"
                    | "status"
                    | "progress"
                    | "task_id"
                    | "weekly_remaining"
                    | "five_hour_remaining"
                    | "wait_reason"
                    | "prompt"
                    | "interaction_id"
                    | "short_action"
                    | "long_action"
                    | "pending_wait_count"
            ) {
                return Err(format!("unknown display context field: {key}"));
            }
        }

        fn string_field(
            object: &serde_json::Map<String, Value>,
            name: &str,
        ) -> Result<Option<String>, String> {
            match object.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) if value.chars().count() <= 160 => {
                    Ok(Some(value.clone()))
                }
                Some(Value::String(_)) => Err(format!(
                    "display context field {name} must be at most 160 characters"
                )),
                Some(_) => Err(format!(
                    "display context field {name} must be a string or null"
                )),
            }
        }

        fn percentage_field(
            object: &serde_json::Map<String, Value>,
            name: &str,
        ) -> Result<Option<u8>, String> {
            match object.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(value)) => {
                    let value = value.as_u64().ok_or_else(|| {
                        format!("display context field {name} must be an integer")
                    })?;
                    if value > 100 {
                        return Err(format!(
                            "display context field {name} must be from 0 to 100"
                        ));
                    }
                    Ok(Some(value as u8))
                }
                Some(_) => Err(format!(
                    "display context field {name} must be an integer or null"
                )),
            }
        }

        let task_id = string_field(object, "task_id")?;
        let weekly_remaining = percentage_field(object, "weekly_remaining")?;
        let five_hour_remaining = percentage_field(object, "five_hour_remaining")?;
        let pending_wait_count = percentage_field(object, "pending_wait_count")?;

        let progress = match object.get("progress") {
            None | Some(Value::Null) => None,
            Some(Value::Number(value)) => {
                let value = value
                    .as_u64()
                    .ok_or_else(|| "display context progress must be an integer".to_owned())?;
                if value > 100 {
                    return Err("display context progress must be from 0 to 100".to_owned());
                }
                Some(value as u8)
            }
            Some(_) => return Err("display context progress must be an integer or null".to_owned()),
        };

        Ok(Self {
            project: string_field(object, "project")?,
            task: string_field(object, "task")?,
            model: string_field(object, "model")?,
            effort: string_field(object, "effort")?,
            status: string_field(object, "status")?,
            progress,
            task_id,
            weekly_remaining,
            five_hour_remaining,
            wait_reason: string_field(object, "wait_reason")?,
            prompt: string_field(object, "prompt")?,
            interaction_id: string_field(object, "interaction_id")?,
            short_action: string_field(object, "short_action")?,
            long_action: string_field(object, "long_action")?,
            pending_wait_count,
        })
    }

    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "project": self.project,
            "task": self.task,
            "model": self.model,
            "effort": self.effort,
            "status": self.status,
            "progress": self.progress,
            "task_id": self.task_id,
            "weekly_remaining": self.weekly_remaining,
            "five_hour_remaining": self.five_hour_remaining,
            "wait_reason": self.wait_reason,
            "prompt": self.prompt,
            "interaction_id": self.interaction_id,
            "short_action": self.short_action,
            "long_action": self.long_action,
            "pending_wait_count": self.pending_wait_count,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerKind {
    Ajazz,
    StreamDeckPlus,
    StreamDeckPlusXl,
    StreamDeckXl,
    /// Virtual controller backed by a Stream Deck plugin session over the
    /// daemon TCP protocol. Not selectable from the CLI; created by the
    /// daemon when a plugin hello arrives.
    StreamDeckPlugin,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceSpec {
    pub kind: ControllerKind,
    pub serial: Option<String>,
    pub task_slots: Option<usize>,
}

impl DeviceSpec {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut parts = value.split(',');
        let kind = ControllerKind::parse(parts.next().unwrap_or(""))?;
        let mut serial = None;
        let mut task_slots = None;
        for part in parts {
            let (key, raw) = part
                .split_once('=')
                .ok_or_else(|| format!("--device option must be key=value (got {part})"))?;
            match key {
                "serial" if !raw.is_empty() => serial = Some(raw.to_owned()),
                "task-slots" => {
                    let slots = raw
                        .parse::<usize>()
                        .map_err(|_| "--device task-slots must be an integer".to_owned())?;
                    if slots == 0 || slots > 64 {
                        return Err("--device task-slots must be from 1 to 64".to_owned());
                    }
                    task_slots = Some(slots);
                }
                "serial" => return Err("--device serial must not be empty".to_owned()),
                other => return Err(format!("unknown --device option: {other}")),
            }
        }
        if !kind.is_physical() {
            return Err("--device requires a physical controller".to_owned());
        }
        Ok(Self {
            kind,
            serial,
            task_slots,
        })
    }
}
impl ControllerKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ajazz" => Ok(Self::Ajazz),
            "streamdeck-plus" => Ok(Self::StreamDeckPlus),
            "streamdeck-plus-xl" => Ok(Self::StreamDeckPlusXl),
            "streamdeck-xl" => Ok(Self::StreamDeckXl),
            "none" => Ok(Self::None),
            _ => Err(format!(
                "--controller must be ajazz, streamdeck-plus, streamdeck-plus-xl, streamdeck-xl, or none (got {value})"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ajazz => "ajazz",
            Self::StreamDeckPlus => "streamdeck-plus",
            Self::StreamDeckPlusXl => "streamdeck-plus-xl",
            Self::StreamDeckXl => "streamdeck-xl",
            Self::StreamDeckPlugin => "streamdeck-plugin",
            Self::None => "none",
        }
    }

    pub fn is_physical(self) -> bool {
        !matches!(self, Self::None)
    }
}

pub trait PhysicalController {
    fn kind(&self) -> ControllerKind;
    fn model(&self) -> &'static str;
    fn serial(&self) -> Option<&str>;
    fn poll(&mut self, timeout_ms: i32) -> Result<Vec<PhysicalEvent>, String>;
    fn apply_thread_status(&mut self, parameters: &Value) -> Result<(), String>;
    /// Number of physical LCD keys available for daemon task cards.
    fn task_slot_count(&self) -> usize {
        match self.kind() {
            ControllerKind::Ajazz => 6,
            ControllerKind::StreamDeckPlus => 8,
            ControllerKind::StreamDeckPlusXl | ControllerKind::StreamDeckXl => 8,
            ControllerKind::StreamDeckPlugin => 0,
            ControllerKind::None => 0,
        }
    }
    fn device_id(&self) -> String {
        self.serial()
            .map(|serial| format!("{}:{serial}", self.kind().as_str()))
            .unwrap_or_else(|| self.kind().as_str().to_owned())
    }
    fn apply_task_cards(&mut self, cards: &[Value]) -> Result<(), String> {
        self.apply_thread_status(&Value::Array(cards.to_vec()))
    }
    fn apply_rgb_config(&mut self, parameters: &Value) -> Result<(), String>;
    fn apply_display_context(&mut self, _context: &DisplayContext) -> Result<(), String> {
        Ok(())
    }
    fn shutdown(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_controller_kinds() {
        assert_eq!(
            ControllerKind::parse("ajazz").unwrap(),
            ControllerKind::Ajazz
        );
        assert_eq!(
            ControllerKind::parse("streamdeck-plus").unwrap(),
            ControllerKind::StreamDeckPlus
        );
        assert_eq!(
            ControllerKind::parse("streamdeck-xl").unwrap(),
            ControllerKind::StreamDeckXl
        );
        assert_eq!(ControllerKind::parse("none").unwrap(), ControllerKind::None);
        assert_eq!(
            ControllerKind::parse("streamdeck-plus-xl").unwrap(),
            ControllerKind::StreamDeckPlusXl
        );
        assert!(ControllerKind::parse("streamdeck-xl-2022").is_err());
    }
    #[test]
    fn parses_device_specs_and_capacity_override() {
        let spec = DeviceSpec::parse("streamdeck-plus,serial=ABC123,task-slots=8").unwrap();
        assert_eq!(spec.kind, ControllerKind::StreamDeckPlus);
        assert_eq!(spec.serial.as_deref(), Some("ABC123"));
        assert_eq!(spec.task_slots, Some(8));
        assert!(DeviceSpec::parse("ajazz,task-slots=0").is_err());
        assert!(DeviceSpec::parse("none").is_err());
    }

    #[test]
    fn parses_and_limits_display_context() {
        let context = DisplayContext::from_value(&serde_json::json!({
            "project": "micro-emu",
            "task": "Stream Deck",
            "model": "gpt-5",
            "effort": "high",
            "status": "working",
            "progress": 42
            ,"weekly_remaining": 73
            ,"five_hour_remaining": 28
        }))
        .unwrap();
        assert_eq!(context.progress, Some(42));
        assert_eq!(context.weekly_remaining, Some(73));
        assert_eq!(context.five_hour_remaining, Some(28));
        assert_eq!(context.to_value()["model"], "gpt-5");
        assert!(DisplayContext::from_value(&serde_json::json!({"progress": 101})).is_err());
        assert_eq!(DisplayContext::from_value(&serde_json::json!({"prompt": "secret"})).unwrap().prompt.as_deref(), Some("secret"));
    }
}
