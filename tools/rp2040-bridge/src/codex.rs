use serde_json::{Deserializer, Value, json};

pub const REPORT_ID: u8 = 6;
pub const REPORT_BYTES: usize = 64;
const OPCODE_DATA: u8 = 2;
const MAX_CHUNK_BYTES: usize = 61;

#[derive(Default)]
pub struct CodexDecoder {
    buffer: Vec<u8>,
}

impl CodexDecoder {
    pub fn feed(&mut self, report: &[u8]) -> Result<Vec<Value>, String> {
        if report.len() != REPORT_BYTES || report[0] != REPORT_ID {
            return Err("Codex report must be 64 bytes and start with Report ID 6".to_owned());
        }
        if report[1] != OPCODE_DATA {
            return Err(format!("unsupported Codex opcode 0x{:02X}", report[1]));
        }
        let length = report[2] as usize;
        if !(1..=MAX_CHUNK_BYTES).contains(&length) {
            return Err(format!("invalid Codex chunk length {length}"));
        }
        self.buffer.extend_from_slice(&report[3..3 + length]);
        if self.buffer.len() > 64 * 1024 {
            self.buffer.clear();
            return Err("Codex message buffer exceeded 64 KiB".to_owned());
        }

        self.decode_available()
    }

    fn decode_available(&mut self) -> Result<Vec<Value>, String> {
        let mut messages = Vec::new();
        loop {
            while self
                .buffer
                .first()
                .is_some_and(|byte| *byte == b'\r' || *byte == b'\n')
            {
                self.buffer.remove(0);
            }
            if self.buffer.is_empty() {
                break;
            }

            let mut stream = Deserializer::from_slice(&self.buffer).into_iter::<Value>();
            match stream.next() {
                Some(Ok(message)) => {
                    let consumed = stream.byte_offset();
                    if consumed == 0 {
                        return Err("Codex decoder consumed zero bytes".to_owned());
                    }
                    self.buffer.drain(..consumed);
                    if !message.is_object() {
                        return Err("Codex JSON message must be an object".to_owned());
                    }
                    messages.push(message);
                }
                Some(Err(error)) if error.is_eof() => break,
                Some(Err(error)) => {
                    let consumed = self.invalid_prefix_length();
                    self.buffer.drain(..consumed.max(1));
                    return Err(format!("invalid Codex JSON: {error}"));
                }
                None => break,
            }
        }
        Ok(messages)
    }

    fn invalid_prefix_length(&self) -> usize {
        if let Some(end) = self.buffer.windows(2).position(|window| window == b"\r\n") {
            return end + 2;
        }
        if let Some(start) = self.buffer.iter().position(|byte| *byte == b'{') {
            if start > 0 {
                return start;
            }
            if let Some(next) = self
                .buffer
                .iter()
                .enumerate()
                .skip(1)
                .find_map(|(index, byte)| (*byte == b'{').then_some(index))
            {
                return next;
            }
        }
        self.buffer.len()
    }
}

pub fn frame_json(message: &Value) -> Result<Vec<[u8; REPORT_BYTES]>, String> {
    if !message.is_object() {
        return Err("Codex JSON message must be an object".to_owned());
    }
    let mut payload = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    payload.extend_from_slice(b"\r\n");
    let mut reports = Vec::new();
    for chunk in payload.chunks(MAX_CHUNK_BYTES) {
        let mut report = [0_u8; REPORT_BYTES];
        report[0] = REPORT_ID;
        report[1] = OPCODE_DATA;
        report[2] = chunk.len() as u8;
        report[3..3 + chunk.len()].copy_from_slice(chunk);
        reports.push(report);
    }
    Ok(reports)
}

/// Stable logical commands exposed by the Stream Deck Action Button catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogAction {
    TaskPrevious,
    TaskNext,
    TaskFirst,
    TaskLast,
    TaskOpen,
    TaskRetry,
    TaskInterrupt,
    TaskFork,
    TaskArchive,
    TaskPin,
    TaskUnpin,
    TaskApprove,
    TaskReject,
    TaskCopyPrompt,
    TaskCopyResponse,
    TaskCopyPath,
    AgentNewTask,
    AgentSearch,
    AgentReviewChanges,
    AgentRunTests,
    AgentOpenTerminal,
    AgentOpenBrowser,
    AgentOpenEditor,
    AgentCompactContext,
    AgentSettings,
}

impl CatalogAction {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "task.previous" => Self::TaskPrevious,
            "task.next" => Self::TaskNext,
            "task.first" => Self::TaskFirst,
            "task.last" => Self::TaskLast,
            "task.open" => Self::TaskOpen,
            "task.retry" => Self::TaskRetry,
            "task.interrupt" => Self::TaskInterrupt,
            "task.fork" => Self::TaskFork,
            "task.archive" => Self::TaskArchive,
            "task.pin" => Self::TaskPin,
            "task.unpin" => Self::TaskUnpin,
            "task.approve" => Self::TaskApprove,
            "task.reject" => Self::TaskReject,
            "task.copy-prompt" => Self::TaskCopyPrompt,
            "task.copy-response" => Self::TaskCopyResponse,
            "task.copy-path" => Self::TaskCopyPath,
            "agent.new-task" => Self::AgentNewTask,
            "agent.search" => Self::AgentSearch,
            "agent.review-changes" => Self::AgentReviewChanges,
            "agent.run-tests" => Self::AgentRunTests,
            "agent.open-terminal" => Self::AgentOpenTerminal,
            "agent.open-browser" => Self::AgentOpenBrowser,
            "agent.open-editor" => Self::AgentOpenEditor,
            "agent.compact-context" => Self::AgentCompactContext,
            "agent.settings" => Self::AgentSettings,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskPrevious => "task.previous",
            Self::TaskNext => "task.next",
            Self::TaskFirst => "task.first",
            Self::TaskLast => "task.last",
            Self::TaskOpen => "task.open",
            Self::TaskRetry => "task.retry",
            Self::TaskInterrupt => "task.interrupt",
            Self::TaskFork => "task.fork",
            Self::TaskArchive => "task.archive",
            Self::TaskPin => "task.pin",
            Self::TaskUnpin => "task.unpin",
            Self::TaskApprove => "task.approve",
            Self::TaskReject => "task.reject",
            Self::TaskCopyPrompt => "task.copy-prompt",
            Self::TaskCopyResponse => "task.copy-response",
            Self::TaskCopyPath => "task.copy-path",
            Self::AgentNewTask => "agent.new-task",
            Self::AgentSearch => "agent.search",
            Self::AgentReviewChanges => "agent.review-changes",
            Self::AgentRunTests => "agent.run-tests",
            Self::AgentOpenTerminal => "agent.open-terminal",
            Self::AgentOpenBrowser => "agent.open-browser",
            Self::AgentOpenEditor => "agent.open-editor",
            Self::AgentCompactContext => "agent.compact-context",
            Self::AgentSettings => "agent.settings",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalEvent {
    Button {
        index: u8,
        pressed: bool,
    },
    /// Explicit task-card control from a virtual controller.
    TaskButton {
        index: u8,
        pressed: bool,
    },
    /// Long-press request to select/show or minimize the Codex desktop app.
    TaskToggle {
        index: u8,
    },
    /// Explicit physical Micro command that bypasses task-card routing.
    MicroButton {
        index: u8,
        pressed: bool,
    },
    /// Extended logical action that has no physical Micro position.
    CatalogAction {
        action: CatalogAction,
    },
    /// Advance the selected Codex task through the featured model list.
    ModelCycle,
    EncoderTurn {
        index: u8,
        delta: i8,
    },
    EncoderButton {
        index: u8,
        pressed: bool,
    },
}

#[derive(Default)]
pub struct RadialState {
    levels: [u8; 2],
    directions: [i8; 2],
}

impl RadialState {
    pub fn event(&mut self, event: PhysicalEvent) -> Option<Value> {
        let (key, action, agent) = match event {
            PhysicalEvent::Button { index, pressed }
            | PhysicalEvent::MicroButton { index, pressed }
                if index <= 5 =>
            {
                (format!("AG0{index}"), i32::from(pressed), Some(index))
            }
            PhysicalEvent::Button { index, pressed }
            | PhysicalEvent::MicroButton { index, pressed }
                if index <= 8 =>
            {
                (format!("ACT{:02}", index), i32::from(pressed), None)
            }
            PhysicalEvent::EncoderTurn { index: 0, delta } if delta != 0 => {
                return Some(self.radial_turn(0, if delta > 0 { 0.0 } else { 0.5 }, delta));
            }
            PhysicalEvent::EncoderTurn { index: 2, delta } if delta != 0 => {
                return Some(self.radial_turn(1, if delta > 0 { 0.25 } else { 0.75 }, delta));
            }
            PhysicalEvent::EncoderTurn { index: 1, delta } if delta > 0 => {
                ("ENC_CW".to_owned(), 2, None)
            }
            PhysicalEvent::EncoderTurn { index: 1, delta } if delta < 0 => {
                ("ENC_CC".to_owned(), 2, None)
            }
            PhysicalEvent::EncoderButton { index: 0, pressed } => {
                ("ACT12".to_owned(), i32::from(pressed), None)
            }
            PhysicalEvent::EncoderButton { index: 2, pressed } => {
                ("ACT10".to_owned(), i32::from(pressed), None)
            }
            PhysicalEvent::EncoderButton { index: 1, pressed } => {
                ("ENC_CLK".to_owned(), i32::from(pressed), None)
            }
            _ => return None,
        };
        let mut parameters = serde_json::Map::new();
        parameters.insert("k".to_owned(), Value::String(key));
        parameters.insert("act".to_owned(), Value::from(action));
        if let Some(agent) = agent {
            parameters.insert("ag".to_owned(), Value::from(agent));
        }
        Some(json!({"m": "v.oai.hid", "p": Value::Object(parameters)}))
    }
}

pub fn message_for_physical_event(event: PhysicalEvent) -> Option<Value> {
    RadialState::default().event(event)
}

impl RadialState {
    fn radial_turn(&mut self, axis: usize, angle: f64, delta: i8) -> Value {
        let direction = delta.signum();
        if self.directions[axis] != direction {
            self.levels[axis] = 0;
            self.directions[axis] = direction;
        }
        self.levels[axis] = (self.levels[axis] + 1).min(2);
        radial_message(angle, f64::from(self.levels[axis]) / 2.0)
    }
}

fn radial_message(angle: f64, distance: f64) -> Value {
    json!({"m":"v.oai.rad","p":{"a":angle,"d":(distance).clamp(0.0, 1.0)}})
}

pub fn messages_for_synthetic_key(key: &str) -> Result<Vec<Value>, String> {
    let events = match key {
        "AG00" | "AG01" | "AG02" | "AG03" | "AG04" | "AG05" => {
            let index = key.as_bytes()[3] - b'0';
            vec![
                PhysicalEvent::Button {
                    index,
                    pressed: true,
                },
                PhysicalEvent::Button {
                    index,
                    pressed: false,
                },
            ]
        }
        "ACT06" | "ACT07" | "ACT08" => {
            let index = key.as_bytes()[4] - b'0';
            vec![
                PhysicalEvent::Button {
                    index,
                    pressed: true,
                },
                PhysicalEvent::Button {
                    index,
                    pressed: false,
                },
            ]
        }
        "ENC_CW" => vec![PhysicalEvent::EncoderTurn { index: 0, delta: 1 }],
        "ENC_CC" => vec![PhysicalEvent::EncoderTurn {
            index: 0,
            delta: -1,
        }],
        "ENC_CLK" => vec![
            PhysicalEvent::EncoderButton {
                index: 0,
                pressed: true,
            },
            PhysicalEvent::EncoderButton {
                index: 0,
                pressed: false,
            },
        ],
        _ => {
            return Err(format!(
                "unsupported synthetic key {key}; use AG00-AG05, ACT06-ACT08, \
                 ENC_CW, ENC_CC, or ENC_CLK"
            ));
        }
    };
    Ok(events
        .into_iter()
        .filter_map(message_for_physical_event)
        .collect())
}

pub fn method(message: &Value) -> Option<&str> {
    message
        .get("m")
        .or_else(|| message.get("method"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_reassembles_device_status() {
        let request = json!({"m": "device.status", "id": 7});
        let reports = frame_json(&request).unwrap();
        let mut decoder = CodexDecoder::default();
        let mut messages = Vec::new();
        for report in reports {
            messages.extend(decoder.feed(&report).unwrap());
        }
        assert_eq!(messages, vec![request]);
    }

    #[test]
    fn reassembles_client_json_without_crlf() {
        let request = json!({"method": "device.status", "params": null, "id": 19});
        let payload = serde_json::to_vec(&request).unwrap();
        let mut decoder = CodexDecoder::default();
        let mut messages = Vec::new();
        for chunk in payload.chunks(MAX_CHUNK_BYTES) {
            let mut report = [0_u8; REPORT_BYTES];
            report[0] = REPORT_ID;
            report[1] = OPCODE_DATA;
            report[2] = chunk.len() as u8;
            report[3..3 + chunk.len()].copy_from_slice(chunk);
            messages.extend(decoder.feed(&report).unwrap());
        }
        assert_eq!(messages, vec![request]);
    }

    #[test]
    fn extracts_concatenated_json_without_crlf() {
        let first = json!({"m": "one"});
        let second = json!({"m": "two"});
        let mut payload = serde_json::to_vec(&first).unwrap();
        payload.extend(serde_json::to_vec(&second).unwrap());
        let mut report = [0_u8; REPORT_BYTES];
        report[0] = REPORT_ID;
        report[1] = OPCODE_DATA;
        report[2] = payload.len() as u8;
        report[3..3 + payload.len()].copy_from_slice(&payload);
        assert_eq!(
            CodexDecoder::default().feed(&report).unwrap(),
            vec![first, second]
        );
    }

    #[test]
    fn maps_six_lcd_keys_and_bottom_buttons() {
        assert_eq!(
            message_for_physical_event(PhysicalEvent::Button {
                index: 0,
                pressed: true
            }),
            Some(json!({"m":"v.oai.hid","p":{"k":"AG00","act":1,"ag":0}}))
        );
        assert_eq!(
            message_for_physical_event(PhysicalEvent::Button {
                index: 8,
                pressed: false
            }),
            Some(json!({"m":"v.oai.hid","p":{"k":"ACT08","act":0}}))
        );
    }

    #[test]
    fn maps_encoder_directions_and_click() {
        assert_eq!(
            message_for_physical_event(PhysicalEvent::EncoderTurn {
                index: 2,
                delta: -1
            }),
            Some(json!({"m":"v.oai.rad","p":{"a":0.75,"d":0.5}}))
        );
        assert_eq!(
            message_for_physical_event(PhysicalEvent::EncoderButton {
                index: 1,
                pressed: true
            }),
            Some(json!({"m":"v.oai.hid","p":{"k":"ENC_CLK","act":1}}))
        );
    }

    #[test]
    fn builds_synthetic_press_release_pair() {
        assert_eq!(
            messages_for_synthetic_key("AG05").unwrap(),
            vec![
                json!({"m":"v.oai.hid","p":{"k":"AG05","act":1,"ag":5}}),
                json!({"m":"v.oai.hid","p":{"k":"AG05","act":0,"ag":5}})
            ]
        );
        assert!(messages_for_synthetic_key("INVALID").is_err());
    }
}

#[cfg(test)]
mod stream_deck_mapping_tests {
    use super::*;

    #[test]
    fn reserved_fourth_encoder_does_not_emit_codex_events() {
        assert!(
            message_for_physical_event(PhysicalEvent::EncoderTurn { index: 3, delta: 1 }).is_none()
        );
        assert!(
            message_for_physical_event(PhysicalEvent::EncoderButton {
                index: 3,
                pressed: true
            })
            .is_none()
        );
    }
}
