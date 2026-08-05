//! Agent routing, event partitioning and LCD slot fusion.
//!
//! The bridge serves two agents simultaneously: Codex (ChatGPT via HID + MCP)
//! and Hermes (MCP only). Physical controller events are partitioned so each
//! agent only sees its half of the keys, and LCD status slots are fused so
//! each agent only writes its assigned slots.

use serde_json::{Value, json};
use std::collections::VecDeque;

/// Number of LCD status slots managed by `v.oai.thstatus`.
pub const LCD_SLOTS: usize = 6;

/// First slot (0-based) owned by Hermes.
pub const HERMES_SLOT_OFFSET: usize = 3;
/// Number of slots owned by each agent.
pub const SLOTS_PER_AGENT: usize = 3;

/// Maximum buffered events per agent before the oldest are dropped.
pub const EVENT_QUEUE_CAPACITY: usize = 256;

/// Identifies which agent a session represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentId {
    Codex,
    Hermes,
}

impl AgentId {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "codex" => Ok(Self::Codex),
            "hermes" => Ok(Self::Hermes),
            _ => Err(format!(
                "agent must be codex or hermes (got {value})"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Hermes => "hermes",
        }
    }

    /// 0-based slot range owned by this agent.
    pub fn slot_range(self) -> std::ops::Range<usize> {
        match self {
            Self::Codex => 0..SLOTS_PER_AGENT,
            Self::Hermes => HERMES_SLOT_OFFSET..HERMES_SLOT_OFFSET + SLOTS_PER_AGENT,
        }
    }
}

/// Which agent owns a physical button index (0-based LCD key index).
pub fn button_owner(index: u8) -> Option<AgentId> {
    match index {
        0..=2 => Some(AgentId::Codex),
        3..=5 => Some(AgentId::Hermes),
        _ => None,
    }
}

/// A buffered event ready to be polled by an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferedEvent {
    pub key: String,
    pub pressed: bool,
    pub timestamp_ms: u128,
}

/// Per-agent event queue with a hard cap; oldest events are dropped first.
#[derive(Default)]
pub struct EventQueue {
    queue: VecDeque<BufferedEvent>,
}

impl EventQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::with_capacity(EVENT_QUEUE_CAPACITY),
        }
    }

    pub fn push(&mut self, event: BufferedEvent) {
        if self.queue.len() >= EVENT_QUEUE_CAPACITY {
            self.queue.pop_front();
        }
        self.queue.push_back(event);
    }

    pub fn drain(&mut self) -> Vec<BufferedEvent> {
        self.queue.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// Holds the event queues for both agents.
pub struct EventRouting {
    pub codex: EventQueue,
    pub hermes: EventQueue,
}

impl EventRouting {
    pub fn new() -> Self {
        Self {
            codex: EventQueue::new(),
            hermes: EventQueue::new(),
        }
    }

    pub fn queue_mut(&mut self, agent: AgentId) -> &mut EventQueue {
        match agent {
            AgentId::Codex => &mut self.codex,
            AgentId::Hermes => &mut self.hermes,
        }
    }

    pub fn queue(&self, agent: AgentId) -> &EventQueue {
        match agent {
            AgentId::Codex => &self.codex,
            AgentId::Hermes => &self.hermes,
        }
    }

    /// Routes a physical button event. Returns `Some(agent)` when the event
    /// belongs to an agent's partition (the caller decides what to do with
    /// it: Codex events go to HID when the RP2040 is present, otherwise they
    /// are buffered for the Codex MCP session; Hermes events are always
    /// buffered).
    pub fn route_button(&mut self, index: u8, pressed: bool, timestamp_ms: u128) {
        let Some(owner) = button_owner(index) else {
            return;
        };
        let key = format!("AG0{index}");
        self.queue_mut(owner).push(BufferedEvent {
            key,
            pressed,
            timestamp_ms,
        });
    }
}

/// Fused LCD state: six slots, each optionally set by an agent.
/// Slot values are stored as-is from `v.oai.thstatus` entries.
#[derive(Clone, Debug, Default)]
pub struct FusedLcdState {
    slots: [Option<Value>; LCD_SLOTS],
}

impl FusedLcdState {
    pub fn new() -> Self {
        Self {
            slots: Default::default(),
        }
    }

    /// Merges a `v.oai.thstatus` payload coming from `agent`, keeping only
    /// that agent's slot range. `parameters` is the array of slot entries.
    /// Returns the fused array suitable for `apply_thread_status`.
    pub fn merge_from_agent(&mut self, agent: AgentId, parameters: &Value) -> Result<Vec<Value>, String> {
        let entries = parameters
            .as_array()
            .ok_or_else(|| "thstatus payload must be an array".to_owned())?;
        let range = agent.slot_range();
        for (i, entry) in entries.iter().enumerate() {
            let slot = range.start + i;
            if slot >= range.end {
                break;
            }
            self.slots[slot] = Some(entry.clone());
        }
        Ok(self.fused_array())
    }

    /// Returns the full fused array of six slot entries. Missing slots are
    /// represented as `{"e":0}` (OFF), matching the existing AJAZZ behavior
    /// where an inactive slot is cleared.
    pub fn fused_array(&self) -> Vec<Value> {
        self.slots
            .iter()
            .map(|slot| slot.clone().unwrap_or_else(|| json!({"e": 0})))
            .collect()
    }

    /// Replaces the entire state from a full six-slot array (used when
    /// replaying after a controller reconnect).
    pub fn replace_full(&mut self, entries: &[Value]) {
        for (i, entry) in entries.iter().enumerate().take(LCD_SLOTS) {
            self.slots[i] = Some(entry.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partitions_buttons_between_agents() {
        assert_eq!(button_owner(0), Some(AgentId::Codex));
        assert_eq!(button_owner(2), Some(AgentId::Codex));
        assert_eq!(button_owner(3), Some(AgentId::Hermes));
        assert_eq!(button_owner(5), Some(AgentId::Hermes));
        assert_eq!(button_owner(6), None);
    }

    #[test]
    fn routes_buttons_to_the_right_queue() {
        let mut routing = EventRouting::new();
        routing.route_button(1, true, 100);
        routing.route_button(4, true, 200);
        assert_eq!(routing.codex.len(), 1);
        assert_eq!(routing.hermes.len(), 1);
        let codex_events = routing.codex.drain();
        assert_eq!(codex_events[0].key, "AG01");
        assert!(codex_events[0].pressed);
        let hermes_events = routing.hermes.drain();
        assert_eq!(hermes_events[0].key, "AG04");
    }

    #[test]
    fn event_queue_drops_oldest_when_full() {
        let mut queue = EventQueue::new();
        for i in 0..(EVENT_QUEUE_CAPACITY + 10) {
            queue.push(BufferedEvent {
                key: format!("AG0{}", i % 6),
                pressed: true,
                timestamp_ms: i as u128,
            });
        }
        assert_eq!(queue.len(), EVENT_QUEUE_CAPACITY);
        let events = queue.drain();
        assert_eq!(events[0].timestamp_ms, 10);
    }

    #[test]
    fn fused_lcd_merges_only_agent_slots() {
        let mut lcd = FusedLcdState::new();
        let codex_status = json!([
            {"i": 0, "e": 1, "t": "codex-0"},
            {"i": 1, "e": 1, "t": "codex-1"},
            {"i": 2, "e": 1, "t": "codex-2"}
        ]);
        lcd.merge_from_agent(AgentId::Codex, &codex_status).unwrap();
        let hermes_status = json!([
            {"i": 0, "e": 1, "t": "hermes-3"},
            {"i": 1, "e": 1, "t": "hermes-4"},
            {"i": 2, "e": 1, "t": "hermes-5"}
        ]);
        lcd.merge_from_agent(AgentId::Hermes, &hermes_status).unwrap();
        let fused = lcd.fused_array();
        assert_eq!(fused.len(), LCD_SLOTS);
        assert_eq!(fused[0]["t"], "codex-0");
        assert_eq!(fused[2]["t"], "codex-2");
        assert_eq!(fused[3]["t"], "hermes-3");
        assert_eq!(fused[5]["t"], "hermes-5");
    }

    #[test]
    fn fused_lcd_ignores_out_of_range_entries() {
        let mut lcd = FusedLcdState::new();
        // Codex sends 5 entries but only slots 0-2 are kept.
        let codex_status = json!([
            {"t": "a"}, {"t": "b"}, {"t": "c"}, {"t": "d"}, {"t": "e"}
        ]);
        lcd.merge_from_agent(AgentId::Codex, &codex_status).unwrap();
        let fused = lcd.fused_array();
        assert_eq!(fused[0]["t"], "a");
        assert_eq!(fused[2]["t"], "c");
        // Slots 3-5 untouched -> OFF.
        assert_eq!(fused[3], json!({"e": 0}));
    }

    #[test]
    fn agent_id_parses_and_rejects_unknown() {
        assert_eq!(AgentId::parse("codex").unwrap(), AgentId::Codex);
        assert_eq!(AgentId::parse("hermes").unwrap(), AgentId::Hermes);
        assert!(AgentId::parse("claude").is_err());
    }
}
