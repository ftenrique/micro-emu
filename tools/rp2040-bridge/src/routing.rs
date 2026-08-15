//! Agent routing, dynamic event partitioning and LCD slot fusion.
//!
//! The bridge serves up to three agents simultaneously: Codex (ChatGPT via
//! HID + MCP), ZCode (Z.ai ADE, MCP only) and Hermes (Nous Research desktop
//! agent, MCP only). Physical controller events are partitioned dynamically
//! based on which agents are currently active, and LCD status slots are fused
//! so each agent only writes its assigned slots.

use serde_json::{Value, json};
use std::collections::VecDeque;

/// Number of LCD status slots managed by `v.oai.thstatus`.
pub const LCD_SLOTS: usize = 6;

/// Number of agents supported by the bridge.
pub const AGENT_COUNT: usize = 3;

/// Maximum buffered events per agent before the oldest are dropped.
pub const EVENT_QUEUE_CAPACITY: usize = 256;

/// Agents in priority order. Used for partition computation and repartition
/// event ordering. Codex has the highest priority, then ZCode, then Hermes.
pub const AGENTS: [AgentId; AGENT_COUNT] = [AgentId::Codex, AgentId::ZCode, AgentId::Hermes];

/// Identifies which agent a session represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentId {
    Codex,
    ZCode,
    Hermes,
}

impl AgentId {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "codex" => Ok(Self::Codex),
            "zcode" => Ok(Self::ZCode),
            "hermes" => Ok(Self::Hermes),
            _ => Err(format!(
                "agent must be codex, zcode, or hermes (got {value})"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ZCode => "zcode",
            Self::Hermes => "hermes",
        }
    }

    /// Index into arrays sized by `AGENT_COUNT`.
    pub fn index(self) -> usize {
        match self {
            Self::Codex => 0,
            Self::ZCode => 1,
            Self::Hermes => 2,
        }
    }
}

/// A set of active agents, stored as a bitmask for efficient
/// insert/remove/contains operations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActiveSet {
    bits: u8,
}

impl ActiveSet {
    pub fn new() -> Self {
        Self { bits: 0 }
    }

    pub fn from_single(agent: AgentId) -> Self {
        let mut set = Self::new();
        set.insert(agent);
        set
    }

    pub fn insert(&mut self, agent: AgentId) {
        self.bits |= 1 << agent.index();
    }

    pub fn remove(&mut self, agent: AgentId) {
        self.bits &= !(1 << agent.index());
    }

    pub fn contains(&self, agent: AgentId) -> bool {
        self.bits & (1 << agent.index()) != 0
    }

    pub fn len(&self) -> usize {
        self.bits.count_ones() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Returns the active agents in priority order.
    pub fn iter(&self) -> Vec<AgentId> {
        AGENTS
            .iter()
            .copied()
            .filter(|a| self.contains(*a))
            .collect()
    }
}

/// The current partition of LCD keys and slots among agents.
/// `owners[i]` is the agent that owns key `AG0i` and LCD slot `i`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Partition {
    owners: [Option<AgentId>; LCD_SLOTS],
}

impl Partition {
    pub fn empty() -> Self {
        Self {
            owners: [None; LCD_SLOTS],
        }
    }

    /// Computes the partition for the given active set.
    ///
    /// Codex always owns the fixed first half (keys AG00-AG02 / slots 0-2).
    /// ZCode has priority for the fixed second half (keys AG03-AG05 /
    /// slots 3-5); Hermes uses that same second half only while ZCode is
    /// absent. Unused halves remain unowned so presses are dropped and cards
    /// render off.
    pub fn compute(active: ActiveSet) -> Self {
        let mut owners = [None; LCD_SLOTS];

        if active.contains(AgentId::Codex) {
            owners[..3].fill(Some(AgentId::Codex));
        }
        if active.contains(AgentId::ZCode) {
            owners[3..].fill(Some(AgentId::ZCode));
        } else if active.contains(AgentId::Hermes) {
            owners[3..].fill(Some(AgentId::Hermes));
        }

        Self { owners }
    }
    /// Returns the agent that owns the given key/slot index, if any.
    pub fn owner_of(&self, index: u8) -> Option<AgentId> {
        self.owners.get(index as usize).copied().flatten()
    }

    /// Returns the absolute slot indices owned by the given agent.
    pub fn slots_for(&self, agent: AgentId) -> Vec<usize> {
        self.owners
            .iter()
            .enumerate()
            .filter_map(|(i, &owner)| if owner == Some(agent) { Some(i) } else { None })
            .collect()
    }

    /// Returns the key labels (e.g. "AG01") owned by the given agent.
    pub fn keys_for(&self, agent: AgentId) -> Vec<String> {
        self.slots_for(agent)
            .iter()
            .map(|&i| format!("AG0{i}"))
            .collect()
    }

    /// Returns the list of owner agent names, one per slot.
    pub fn owners_json(&self) -> Vec<Value> {
        self.owners
            .iter()
            .map(|owner| match owner {
                Some(agent) => json!(agent.as_str()),
                None => Value::Null,
            })
            .collect()
    }
}

/// A buffered event ready to be polled by an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferedEvent {
    /// A physical key press/release.
    Key {
        key: String,
        pressed: bool,
        timestamp_ms: u128,
    },
    /// A partition change notification. Tells the agent which keys/slots it
    /// now owns and which agents are currently active.
    Partition {
        keys: Vec<String>,
        slots: Vec<usize>,
        agents: Vec<String>,
        timestamp_ms: u128,
    },
}

impl BufferedEvent {
    /// Returns the timestamp of the event regardless of variant.
    pub fn timestamp_ms(&self) -> u128 {
        match self {
            Self::Key { timestamp_ms, .. } | Self::Partition { timestamp_ms, .. } => *timestamp_ms,
        }
    }

    /// Serializes the event to a JSON value suitable for `poll_events`.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Key {
                key,
                pressed,
                timestamp_ms,
            } => json!({
                "type": "key",
                "key": key,
                "pressed": pressed,
                "ts": timestamp_ms
            }),
            Self::Partition {
                keys,
                slots,
                agents,
                timestamp_ms,
            } => json!({
                "type": "partition",
                "keys": keys,
                "slots": slots,
                "agents": agents,
                "ts": timestamp_ms
            }),
        }
    }
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

/// Holds the event queues for all agents.
pub struct EventRouting {
    queues: [EventQueue; AGENT_COUNT],
}

impl EventRouting {
    pub fn new() -> Self {
        Self {
            queues: [EventQueue::new(), EventQueue::new(), EventQueue::new()],
        }
    }

    pub fn queue_mut(&mut self, agent: AgentId) -> &mut EventQueue {
        &mut self.queues[agent.index()]
    }

    pub fn queue(&self, agent: AgentId) -> &EventQueue {
        &self.queues[agent.index()]
    }

    /// Routes a physical button event to the owning agent's queue based on
    /// the current partition. Unowned buttons are silently dropped.
    pub fn route_button(
        &mut self,
        index: u8,
        pressed: bool,
        timestamp_ms: u128,
        partition: &Partition,
    ) {
        let Some(owner) = partition.owner_of(index) else {
            return;
        };
        let key = format!("AG0{index}");
        self.queue_mut(owner).push(BufferedEvent::Key {
            key,
            pressed,
            timestamp_ms,
        });
    }

    /// Pushes a partition change notification to an agent's queue.
    pub fn push_partition_event(
        &mut self,
        agent: AgentId,
        partition: &Partition,
        active_agents: &[AgentId],
        timestamp_ms: u128,
    ) {
        let event = BufferedEvent::Partition {
            keys: partition.keys_for(agent),
            slots: partition.slots_for(agent),
            agents: active_agents
                .iter()
                .map(|a| a.as_str().to_owned())
                .collect(),
            timestamp_ms,
        };
        self.queue_mut(agent).push(event);
    }
}

/// Fused LCD state: each agent stores up to `LCD_SLOTS` entries in a
/// local buffer indexed by the position in the array that agent sent.
/// The fused array is rendered through the current partition, so a
/// repartition is non-destructive — previously sent entries are retained
/// and reappear when the agent's slot set grows again.
#[derive(Clone, Debug, Default)]
pub struct FusedLcdState {
    entries: [[Option<Value>; LCD_SLOTS]; AGENT_COUNT],
}

impl FusedLcdState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merges a `v.oai.thstatus` payload from `agent`, storing entries in
    /// the agent's local buffer (up to `LCD_SLOTS`). Returns the fused
    /// array rendered through the current partition.
    pub fn merge_from_agent(
        &mut self,
        agent: AgentId,
        parameters: &Value,
        partition: &Partition,
    ) -> Result<Vec<Value>, String> {
        let entries = parameters
            .as_array()
            .ok_or_else(|| "thstatus payload must be an array".to_owned())?;
        let agent_idx = agent.index();
        for (i, entry) in entries.iter().enumerate().take(LCD_SLOTS) {
            self.entries[agent_idx][i] = Some(entry.clone());
        }
        Ok(self.fused_array(partition))
    }

    /// Returns the full fused array of six slot entries rendered through
    /// the current partition. Missing or unowned slots are represented as
    /// `{"e":0}` (OFF), matching the existing AJAZZ behavior where an
    /// inactive slot is cleared.
    pub fn fused_array(&self, partition: &Partition) -> Vec<Value> {
        (0..LCD_SLOTS)
            .map(|s| match partition.owner_of(s as u8) {
                Some(agent) => {
                    let slots = partition.slots_for(agent);
                    let local_index = slots.iter().position(|&slot| slot == s);
                    match local_index {
                        Some(li) => {
                            let Some(mut entry) = self.entries[agent.index()][li].clone() else {
                                return json!({"e": 0});
                            };
                            normalize_render_entry(&mut entry, s, agent);
                            entry
                        }
                        None => json!({"e": 0}),
                    }
                }
                None => json!({"e": 0}),
            })
            .collect()
    }

    /// Replaces the entire state from a full six-slot array, distributing
    /// entries to the owning agents' local buffers via the current partition.
    /// Used when replaying after a controller reconnect.
    pub fn replace_full(&mut self, entries: &[Value], partition: &Partition) {
        for (i, entry) in entries.iter().enumerate().take(LCD_SLOTS) {
            if let Some(owner) = partition.owner_of(i as u8) {
                let slots = partition.slots_for(owner);
                if let Some(local_index) = slots.iter().position(|&slot| slot == i) {
                    self.entries[owner.index()][local_index] = Some(entry.clone());
                }
            }
        }
    }
}
fn normalize_render_entry(entry: &mut Value, slot: usize, agent: AgentId) {
    if let Some(object) = entry.as_object_mut() {
        object.insert("id".to_owned(), Value::from(slot));
        object.insert("i".to_owned(), Value::from(slot));
        object.insert("agent".to_owned(), Value::from(agent.as_str()));
        if let Some(value) = object.get("b").and_then(Value::as_f64) {
            if value > 1.0 {
                object.insert("b".to_owned(), Value::from((value / 100.0).clamp(0.0, 1.0)));
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_parses_three_agents() {
        assert_eq!(AgentId::parse("codex"), Ok(AgentId::Codex));
        assert_eq!(AgentId::parse("zcode"), Ok(AgentId::ZCode));
        assert_eq!(AgentId::parse("hermes"), Ok(AgentId::Hermes));
        assert!(AgentId::parse("unknown").is_err());
    }

    #[test]
    fn agent_index_is_consistent() {
        assert_eq!(AgentId::Codex.index(), 0);
        assert_eq!(AgentId::ZCode.index(), 1);
        assert_eq!(AgentId::Hermes.index(), 2);
    }

    #[test]
    fn active_set_tracks_agents() {
        let mut set = ActiveSet::new();
        assert!(set.is_empty());
        set.insert(AgentId::Codex);
        set.insert(AgentId::Hermes);
        assert_eq!(set.len(), 2);
        assert!(set.contains(AgentId::Codex));
        assert!(!set.contains(AgentId::ZCode));
        assert!(set.contains(AgentId::Hermes));
        assert_eq!(set.iter(), vec![AgentId::Codex, AgentId::Hermes]);
        set.remove(AgentId::Codex);
        assert_eq!(set.iter(), vec![AgentId::Hermes]);
    }

    #[test]
    fn partition_no_active_agents() {
        let partition = Partition::compute(ActiveSet::new());
        for i in 0..6 {
            assert_eq!(partition.owner_of(i), None);
        }
    }

    #[test]
    fn single_agents_keep_their_fixed_half() {
        let codex = Partition::compute(ActiveSet::from_single(AgentId::Codex));
        assert_eq!(codex.slots_for(AgentId::Codex), vec![0, 1, 2]);
        assert_eq!(codex.owner_of(3), None);

        for agent in [AgentId::ZCode, AgentId::Hermes] {
            let partition = Partition::compute(ActiveSet::from_single(agent));
            assert_eq!(partition.slots_for(agent), vec![3, 4, 5]);
            assert_eq!(partition.owner_of(0), None);
        }
    }
    #[test]
    fn partition_two_agents_split_in_half() {
        let mut set = ActiveSet::new();
        set.insert(AgentId::Codex);
        set.insert(AgentId::Hermes);
        let partition = Partition::compute(set);
        assert_eq!(partition.owner_of(0), Some(AgentId::Codex));
        assert_eq!(partition.owner_of(2), Some(AgentId::Codex));
        assert_eq!(partition.owner_of(3), Some(AgentId::Hermes));
        assert_eq!(partition.owner_of(5), Some(AgentId::Hermes));
        assert_eq!(partition.slots_for(AgentId::Codex), vec![0, 1, 2]);
        assert_eq!(partition.slots_for(AgentId::Hermes), vec![3, 4, 5]);
    }

    #[test]
    fn partition_zcode_is_fixed_to_second_half() {
        let mut set = ActiveSet::new();
        set.insert(AgentId::ZCode);
        set.insert(AgentId::Hermes);
        let partition = Partition::compute(set);
        assert_eq!(partition.slots_for(AgentId::ZCode), vec![3, 4, 5]);
        assert!(partition.slots_for(AgentId::Hermes).is_empty());
        assert_eq!(partition.owner_of(0), None);
    }

    #[test]
    fn partition_codex_and_zcode_use_fixed_halves() {
        let mut set = ActiveSet::new();
        set.insert(AgentId::Codex);
        set.insert(AgentId::ZCode);
        set.insert(AgentId::Hermes);
        let partition = Partition::compute(set);
        assert_eq!(partition.slots_for(AgentId::Codex), vec![0, 1, 2]);
        assert_eq!(partition.slots_for(AgentId::ZCode), vec![3, 4, 5]);
        assert!(partition.slots_for(AgentId::Hermes).is_empty());
        assert_eq!(
            partition.keys_for(AgentId::ZCode),
            vec!["AG03".to_owned(), "AG04".to_owned(), "AG05".to_owned()]
        );
    }
    #[test]
    fn routes_buttons_to_the_right_queue() {
        let partition = Partition::compute(ActiveSet::from_single(AgentId::Codex));
        let mut routing = EventRouting::new();
        routing.route_button(1, true, 100, &partition);
        routing.route_button(4, true, 200, &partition); // Reserved for the second half.
        assert_eq!(routing.queue(AgentId::Codex).len(), 1);
        let events = routing.queue_mut(AgentId::Codex).drain();
        assert_eq!(
            events[0],
            BufferedEvent::Key {
                key: "AG01".to_owned(),
                pressed: true,
                timestamp_ms: 100
            }
        );
    }

    #[test]
    fn event_queue_drops_oldest_when_full() {
        let mut queue = EventQueue::new();
        for i in 0..(EVENT_QUEUE_CAPACITY + 10) {
            queue.push(BufferedEvent::Key {
                key: format!("AG0{}", i % 6),
                pressed: true,
                timestamp_ms: i as u128,
            });
        }
        assert_eq!(queue.len(), EVENT_QUEUE_CAPACITY);
        let events = queue.drain();
        assert_eq!(events[0].timestamp_ms(), 10);
    }

    #[test]
    fn fused_lcd_single_agent_renders_all_six() {
        let partition = Partition::compute(ActiveSet::from_single(AgentId::Codex));
        let mut lcd = FusedLcdState::new();
        let status = json!([
            {"i": 0, "e": 1, "t": "codex-0"},
            {"i": 1, "e": 1, "t": "codex-1"},
            {"i": 2, "e": 1, "t": "codex-2"}
        ]);
        lcd.merge_from_agent(AgentId::Codex, &status, &partition)
            .unwrap();
        let fused = lcd.fused_array(&partition);
        assert_eq!(fused.len(), 6);
        assert_eq!(fused[0]["t"], "codex-0");
        assert_eq!(fused[1]["t"], "codex-1");
        assert_eq!(fused[2]["t"], "codex-2");
        // Slots 3-5 are unset → {"e":0}.
        assert_eq!(fused[3], json!({"e": 0}));
    }

    #[test]
    fn fused_lcd_two_agents_render_only_owned_slots() {
        let mut set = ActiveSet::new();
        set.insert(AgentId::Codex);
        set.insert(AgentId::Hermes);
        let partition = Partition::compute(set);

        let mut lcd = FusedLcdState::new();
        let codex_status = json!([
            {"i": 0, "e": 1, "t": "codex-0"},
            {"i": 1, "e": 1, "t": "codex-1"},
            {"i": 2, "e": 1, "t": "codex-2"}
        ]);
        lcd.merge_from_agent(AgentId::Codex, &codex_status, &partition)
            .unwrap();
        let hermes_status = json!([
            {"i": 0, "e": 1, "t": "hermes-3"},
            {"i": 1, "e": 1, "t": "hermes-4"},
            {"i": 2, "e": 1, "t": "hermes-5"}
        ]);
        lcd.merge_from_agent(AgentId::Hermes, &hermes_status, &partition)
            .unwrap();
        let fused = lcd.fused_array(&partition);
        assert_eq!(fused[0]["t"], "codex-0");
        assert_eq!(fused[1]["t"], "codex-1");
        assert_eq!(fused[2]["t"], "codex-2");
        assert_eq!(fused[3]["t"], "hermes-3");
        assert_eq!(fused[4]["t"], "hermes-4");
        assert_eq!(fused[5]["t"], "hermes-5");
    }

    #[test]
    fn fused_lcd_keeps_codex_first_half_when_zcode_joins() {
        let codex_only = Partition::compute(ActiveSet::from_single(AgentId::Codex));
        let mut lcd = FusedLcdState::new();
        let status = json!([
            {"i": 0, "e": 1, "t": "c0"},
            {"i": 1, "e": 1, "t": "c1"},
            {"i": 2, "e": 1, "t": "c2"}
        ]);
        lcd.merge_from_agent(AgentId::Codex, &status, &codex_only)
            .unwrap();

        let mut set = ActiveSet::new();
        set.insert(AgentId::Codex);
        set.insert(AgentId::ZCode);
        let fixed = Partition::compute(set);
        let fused = lcd.fused_array(&fixed);
        assert_eq!(fused[0]["t"], "c0");
        assert_eq!(fused[2]["t"], "c2");
        assert_eq!(fused[3], json!({"e": 0})); // ZCode has not painted.
    }
    #[test]
    fn fused_lcd_fixed_codex_zcode_halves_use_local_indexing() {
        let mut set = ActiveSet::new();
        set.insert(AgentId::Codex);
        set.insert(AgentId::ZCode);
        set.insert(AgentId::Hermes);
        let partition = Partition::compute(set);

        let mut lcd = FusedLcdState::new();
        // Codex sends 2 entries → local 0 and 1 → slots 0 and 3.
        let codex_status = json!([
            {"i": 0, "e": 1, "t": "c-a"},
            {"i": 1, "e": 1, "t": "c-b"}
        ]);
        lcd.merge_from_agent(AgentId::Codex, &codex_status, &partition)
            .unwrap();
        let fused = lcd.fused_array(&partition);
        assert_eq!(fused[0]["t"], "c-a");
        assert_eq!(fused[1]["t"], "c-b");
        assert_eq!(fused[3], json!({"e": 0})); // ZCode has not painted.
    }

    #[test]
    fn partition_event_serializes_correctly() {
        let event = BufferedEvent::Partition {
            keys: vec!["AG01".to_owned(), "AG04".to_owned()],
            slots: vec![1, 4],
            agents: vec!["codex".to_owned(), "zcode".to_owned(), "hermes".to_owned()],
            timestamp_ms: 12345,
        };
        let json = event.to_json();
        assert_eq!(json["type"], "partition");
        assert_eq!(json["keys"][0], "AG01");
        assert_eq!(json["slots"][1], 4);
        assert_eq!(json["agents"][2], "hermes");
        assert_eq!(json["ts"], 12345);
    }

    #[test]
    fn key_event_serializes_with_type_field() {
        let event = BufferedEvent::Key {
            key: "AG03".to_owned(),
            pressed: true,
            timestamp_ms: 999,
        };
        let json = event.to_json();
        assert_eq!(json["type"], "key");
        assert_eq!(json["key"], "AG03");
        assert_eq!(json["pressed"], true);
        assert_eq!(json["ts"], 999);
    }

    #[test]
    fn push_partition_event_delivers_to_agent_queue() {
        let partition = Partition::compute({
            let mut s = ActiveSet::new();
            s.insert(AgentId::Codex);
            s.insert(AgentId::ZCode);
            s.insert(AgentId::Hermes);
            s
        });
        let active = partition_owners_as_agents(&partition);
        let mut routing = EventRouting::new();
        routing.push_partition_event(AgentId::ZCode, &partition, &active, 100);
        let events = routing.queue_mut(AgentId::ZCode).drain();
        assert_eq!(events.len(), 1);
        match &events[0] {
            BufferedEvent::Partition { keys, slots, .. } => {
                assert_eq!(
                    *keys,
                    vec!["AG03".to_owned(), "AG04".to_owned(), "AG05".to_owned()]
                );
                assert_eq!(*slots, vec![3, 4, 5]);
            }
            _ => panic!("expected partition event"),
        }
    }

    fn partition_owners_as_agents(partition: &Partition) -> Vec<AgentId> {
        AGENTS
            .iter()
            .copied()
            .filter(|a| !partition.slots_for(*a).is_empty())
            .collect()
    }
}
