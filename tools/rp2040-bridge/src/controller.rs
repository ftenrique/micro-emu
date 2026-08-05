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
}

impl DisplayContext {
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "display context must be an object".to_owned())?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "project" | "task" | "model" | "effort" | "status" | "progress"
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
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerKind {
    Ajazz,
    StreamDeckPlus,
    StreamDeckXl,
    None,
}

impl ControllerKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ajazz" => Ok(Self::Ajazz),
            "streamdeck-plus" => Ok(Self::StreamDeckPlus),
            "streamdeck-xl" => Ok(Self::StreamDeckXl),
            "none" => Ok(Self::None),
            _ => Err(format!(
                "--controller must be ajazz, streamdeck-plus, streamdeck-xl, or none (got {value})"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ajazz => "ajazz",
            Self::StreamDeckPlus => "streamdeck-plus",
            Self::StreamDeckXl => "streamdeck-xl",
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
        assert!(ControllerKind::parse("streamdeck-xl-2022").is_err());
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
        }))
        .unwrap();
        assert_eq!(context.progress, Some(42));
        assert_eq!(context.to_value()["model"], "gpt-5");
        assert!(DisplayContext::from_value(&serde_json::json!({"progress": 101})).is_err());
        assert!(DisplayContext::from_value(&serde_json::json!({"prompt": "secret"})).is_err());
    }
}
