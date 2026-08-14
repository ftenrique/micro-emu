//! Task-instance scheduling for daemon mode.
//!
//! The legacy bridge partitioned six physical slots by agent product.  The
//! daemon now treats a task as the schedulable resource and maps tasks onto a
//! combined list of physical device slots.  This module is deliberately
//! controller-neutral; rendering and HID compatibility stay at the callers.

use crate::routing::AgentId;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskAction {
    pub id: String,
    pub label: String,
    pub action: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInteraction {
    pub id: String,
    pub kind: String,
    pub prompt: String,
    pub short: Option<TaskAction>,
    pub long: Option<TaskAction>,
    pub expires_at_ms: Option<u128>,


}
use std::time::Duration;

pub const RECONNECT_GRACE: Duration = Duration::from_secs(30);
/// Number of Codex desktop tasks exposed to the task board. This may exceed
/// the six physical Codex Micro LCD/HID positions when a larger controller or
/// a desktop-only task-card layout is connected.
pub const CODEX_TASK_SLOTS: usize = 9;
/// Number of physical task positions represented by the Codex Micro protocol.
pub const CODEX_HID_SLOTS: usize = 6;

/// Returns the task title as it should appear on the Stream Deck strip.
///
/// The task board keeps the protocol title unchanged; this is only the
/// controller-facing label. Keeping the formatter idempotent matters during
/// reconnect/replay, where the same display context may pass through the
/// render path more than once.
pub fn display_task_title(slot: usize, title: &str) -> String {
    if slot >= CODEX_TASK_SLOTS {
        return title.to_owned();
    }

    let title = title
        .split_once(" \u{2014} ")
        .and_then(|(number, remainder)| {
            number
                .parse::<usize>()
                .ok()
                .filter(|number| (1..=CODEX_TASK_SLOTS).contains(number))
                .map(|_| remainder)
        })
        .unwrap_or(title);
    let prefix = format!("{} \u{2014} ", slot + 1);
    let remaining = 160usize.saturating_sub(prefix.chars().count());
    if title.is_empty() {
        return (slot + 1).to_string();
    }
    format!(
        "{prefix}{}",
        title.chars().take(remaining).collect::<String>()
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskState {
    Queued,
    Running,
    Thinking,
    Waiting,
    Error,
    Paused,
    Completed,
    Reconnecting,
}

impl TaskState {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("queued") {
            "queued" | "idle" | "ready" => Ok(Self::Queued),
            "running" | "active" | "working" => Ok(Self::Running),
            "thinking" => Ok(Self::Thinking),
            "waiting" | "blocked" => Ok(Self::Waiting),
            "error" | "failed" => Ok(Self::Error),
            "paused" => Ok(Self::Paused),
            "completed" | "complete" | "done" => Ok(Self::Completed),
            "reconnecting" => Ok(Self::Reconnecting),
            other => Err(format!("unsupported task state: {other}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Thinking => "thinking",
            Self::Waiting => "waiting",
            Self::Error => "error",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Reconnecting => "reconnecting",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Waiting | Self::Error => 0,
            Self::Running | Self::Thinking => 1,
            Self::Queued | Self::Paused => 2,
            Self::Completed => 3,
            Self::Reconnecting => 4,
        }
    }

    pub fn active(self) -> bool {
        matches!(self, Self::Running | Self::Thinking)
    }

    fn eligible(self) -> bool {
        !matches!(self, Self::Reconnecting)
    }

    pub fn display_color(self) -> u32 {
        match self {
            Self::Queued | Self::Reconnecting => 0x37474f,
            Self::Running => 0x1565c0,
            Self::Thinking => 0x6a1b9a,
            Self::Waiting | Self::Paused => 0xef6c00,
            Self::Error => 0xb71c1c,
            Self::Completed => 0x1b5e20,
        }
    }

    /// Decodes the semantic colors used by Codex's `v.oai.thstatus` feed.
    ///
    /// The effect field describes the LCD animation, not the task lifecycle:
    /// a running card may be either animated or solid. Codex keeps the task
    /// state in this stable palette, which is also used by `color_to_status`
    /// for the touch-strip context.
    fn from_hid_color(color: u32) -> Option<Self> {
        match color {
            0x1565c0 => Some(Self::Running),
            0x6a1b9a => Some(Self::Thinking),
            0xef6c00 => Some(Self::Waiting),
            0xb71c1c => Some(Self::Error),
            0x2e7d32 | 0x1b5e20 => Some(Self::Completed),
            0x0277bd | 0x37474f => Some(Self::Queued),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskCard {
    pub task_id: String,
    pub owner_session: usize,
    pub owner_agent: AgentId,
    pub title: String,
    pub project: Option<String>,
    pub workspace_path: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub state: TaskState,
    pub priority: u8,
    pub color: Option<u32>,
    pub brightness: u8,
    pub progress: Option<u8>,
    pub context: Value,
    pub legacy_key: Option<String>,
    /// Logical AG00-AG05 position supplied by the task source.
    pub source_slot: Option<usize>,
    /// True when lifecycle timestamps came from the task source and must not
    /// be reconstructed from render observations.
    pub timing_authoritative: bool,
    pub updated_at_ms: u128,
    /// When the task first entered its running state, in Unix milliseconds.
    pub started_at_ms: Option<u128>,
    /// When the task completed, in Unix milliseconds. Kept so controllers can
    /// show the final elapsed time instead of resetting the card.
    pub finished_at_ms: Option<u128>,
    pub reconnect_until_ms: Option<u128>,
    pub interaction: Option<TaskInteraction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceSlot {
    pub device_id: String,
    pub slot: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskAssignment {
    pub slot: DeviceSlot,
}

#[derive(Clone, Debug, Default)]
pub struct TaskBoard {
    tasks: HashMap<String, TaskCard>,
    assignments: HashMap<String, TaskAssignment>,
    devices: BTreeMap<String, Vec<DeviceSlot>>,
    /// One selected task for the whole board. Every controller projects this
    /// same identity, so stale per-device highlights cannot accumulate.
    selected: Option<String>,
    /// An optimistic hardware selection remains visible while Codex emits
    /// the corresponding focused-primary-task event in its desktop log.
    pending_selection: Option<(String, u128)>,
    /// Completed tasks acknowledged by selecting them. Their source may keep
    /// publishing the same completed snapshot, so retain the acknowledgement
    /// until that task leaves the completed lifecycle state.
    reviewed_completions: HashSet<String>,
    /// Latest board clock supplied by the daemon. Rendering uses this instead
    /// of wall-clock access so completion recency remains deterministic.
    now_ms: u128,
    /// Owners for slots on the partitioned primary controller. An empty list
    /// preserves the legacy unrestricted scheduler behavior.
    slot_owners: Vec<Option<AgentId>>,
    partitioned_device_id: Option<String>,
    selection_activation_guards: HashMap<String, u128>,
    /// Last slot each task occupied. Used to re-pin a task to its previous
    /// position after a transient eviction so cards do not shuffle.
    last_slots: HashMap<String, DeviceSlot>,
}

impl TaskBoard {
    pub fn new() -> Self {
        Self::default()
    }

    /// A physical task-button press selects a Codex task and can cause the
    /// device to echo one transient active frame. Keep that echo from turning
    /// an idle card into a running timer; genuine later activity still wins.
    pub fn guard_selection_activation(&mut self, task_id: &str, now_ms: u128) {
        if self
            .tasks
            .get(task_id)
            .is_some_and(|task| !task.state.active())
        {
            self.selection_activation_guards.clear();
            self.selection_activation_guards
                .insert(task_id.to_owned(), now_ms.saturating_add(1_000));
        }
    }

    pub fn set_device(&mut self, device_id: impl Into<String>, task_slots: usize, connected: bool) {
        let device_id = device_id.into();
        if connected {
            let slots = (0..task_slots)
                .map(|slot| DeviceSlot {
                    device_id: device_id.clone(),
                    slot,
                })
                .collect();
            self.devices.insert(device_id, slots);
        } else {
            self.devices.remove(&device_id);
            self.assignments
                .retain(|_, assignment| assignment.slot.device_id != device_id);
        }
        self.reallocate();
    }

    pub fn clear_devices(&mut self) {
        self.devices.clear();
        self.assignments.clear();
    }

    /// Applies the current primary-controller partition. Auxiliary controller
    /// slots are deliberately not constrained by this mapping.
    pub fn set_slot_owners(&mut self, device_id: impl Into<String>, owners: Vec<Option<AgentId>>) {
        let device_id = device_id.into();
        if self.partitioned_device_id.as_deref() != Some(device_id.as_str())
            || self.slot_owners != owners
        {
            self.partitioned_device_id = Some(device_id);
            self.slot_owners = owners;
            // Do not clear assignments wholesale: `reallocate` already evicts
            // any assignment that violates the new owner map. Keeping the
            // compliant ones pinned stops cards from jumping between slots
            // every time the partition is recomputed (e.g. when a task-button
            // press wakes the Codex serial traffic back up).
            self.reallocate();
        }
    }
    pub fn publish_tasks(
        &mut self,
        session: usize,
        agent: AgentId,
        value: &Value,
        now_ms: u128,
    ) -> Result<Value, String> {
        let tasks = value
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(|| "publish_tasks requires a tasks array".to_owned())?;
        let mut seen = HashSet::new();
        let mut replacement = Vec::with_capacity(tasks.len());
        for item in tasks {
            replacement.push(self.parse_task(session, agent, item, now_ms, None)?);
            let id = replacement.last().expect("pushed task").task_id.clone();
            if let Some(existing) = self.tasks.get(&id) {
                if existing.owner_session != session {
                    return Err(format!(
                        "task_id is already owned by session {}: {id}",
                        existing.owner_session
                    ));
                }
            }
            if !seen.insert(id.clone()) {
                return Err(format!("duplicate task_id in publish_tasks: {id}"));
            }
        }
        // Keep the complete session snapshot. The scheduler enforces the
        // current partition when assigning physical slots. Destructively
        // truncating here loses tasks published during the repartition
        // debounce, when a newly connected agent temporarily owns zero slots.
        self.replace_session_tasks(session, replacement, now_ms);
        Ok(self.assignments_for_session(session))
    }

    /// Replaces the synthetic Codex task set with authoritative per-thread
    /// records and reconciles one global selection from explicit desktop focus.
    pub fn publish_codex_snapshot(&mut self, value: &Value, now_ms: u128) -> Result<Value, String> {
        let authoritative_selection = value
            .get("selected_task_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let result = self.publish_tasks(0, AgentId::Codex, value, now_ms)?;
        self.reconcile_authoritative_selection(authoritative_selection.as_deref(), now_ms);
        Ok(result)
    }

    fn reconcile_authoritative_selection(&mut self, authoritative: Option<&str>, now_ms: u128) {
        let Some(authoritative) = authoritative.filter(|id| self.tasks.contains_key(*id)) else {
            return;
        };
        if self
            .selected
            .as_deref()
            .and_then(|id| self.tasks.get(id))
            .is_some_and(|task| task.owner_agent != AgentId::Codex)
        {
            return;
        }
        if let Some((pending, deadline)) = self.pending_selection.as_ref() {
            if pending == authoritative {
                self.selected = Some(authoritative.to_owned());
                self.pending_selection = None;
                return;
            }
            if now_ms <= *deadline && self.tasks.contains_key(pending) {
                self.selected = Some(pending.clone());
                return;
            }
        }
        self.selected = Some(authoritative.to_owned());
        self.pending_selection = None;
    }

    /// Adapts the existing six-entry status array to stable session-local
    /// cards.  Disabled entries release their corresponding card.
    pub fn publish_legacy_status(
        &mut self,
        session: usize,
        agent: AgentId,
        value: &Value,
        now_ms: u128,
    ) -> Result<Value, String> {
        let entries = value
            .as_array()
            .ok_or_else(|| "status must be an array".to_owned())?;
        let mut cards = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let enabled = entry.get("e").and_then(Value::as_u64).unwrap_or(1) != 0;
            let task_id = format!("legacy:{session}:{index}");
            if !enabled {
                continue;
            }
            cards.push(self.parse_task(
                session,
                agent,
                entry,
                now_ms,
                Some((task_id, format!("AG0{index}"))),
            )?);
        }
        self.replace_session_tasks(session, cards, now_ms);
        Ok(self.assignments_for_session(session))
    }

    /// Publishes the six logical Codex HID cards.  These are intentionally
    /// owned by the synthetic session rather than by whichever MCP proxy is
    /// currently connected.
    pub fn publish_codex_hid_status(&mut self, value: &Value, now_ms: u128) -> Result<(), String> {
        let entries = value
            .as_array()
            .ok_or_else(|| "thstatus payload must be an array".to_owned())?;
        self.selection_activation_guards
            .retain(|_, expires_at| now_ms <= *expires_at);
        let selection_guard_active = !self.selection_activation_guards.is_empty();
        let session = 0;
        let mut cards = Vec::with_capacity(entries.len());
        for (position, entry) in entries.iter().enumerate().take(CODEX_HID_SLOTS) {
            let index = entry
                .get("id")
                .or_else(|| entry.get("i"))
                .and_then(Value::as_u64)
                .and_then(|slot| usize::try_from(slot).ok())
                .unwrap_or(position);
            if index >= CODEX_HID_SLOTS {
                continue;
            }
            let task_id = format!("codex-hid:{index}");
            let previous = self.tasks.get(&task_id).cloned();
            let effect = entry.get("e").and_then(Value::as_u64);
            let brightness_cleared = entry
                .get("b")
                .and_then(Value::as_f64)
                .is_some_and(|brightness| brightness <= f64::EPSILON);
            let cleared = effect == Some(0) || brightness_cleared;
            // e:0 and b:0 only clear the LCD presentation. They carry no
            // lifecycle meaning and must never finish or reset a task.
            if cleared {
                continue;
            }
            let card = self.parse_task(
                session,
                AgentId::Codex,
                entry,
                now_ms,
                Some((task_id.clone(), format!("AG0{index}"))),
            )?;
            let observed_state = card.state;
            cards.push((previous, observed_state, card));
        }

        // Codex redraws task cards when the user changes the selected thread.
        // That presentation-only frame promotes the newly selected idle card
        // and demotes the genuinely running card, even though neither task's
        // lifecycle changed. Detect the complete queued/running handoff so a
        // selection made in the Codex UI (and therefore lacking a local button
        // guard) cannot be mistaken for execution state.
        let selection_handoff = cards.iter().any(|(previous, observed, _)| {
            previous
                .as_ref()
                .is_some_and(|task| task.state.active() && *observed == TaskState::Queued)
        }) && cards.iter().any(|(previous, observed, _)| {
            previous
                .as_ref()
                .is_some_and(|task| task.state == TaskState::Queued && observed.active())
        }) && cards.iter().all(|(previous, observed, _)| {
            previous.as_ref().is_none_or(|task| {
                task.state == *observed || is_selection_transition(task.state, *observed)
            })
        });

        if selection_handoff {
            let selected_card = cards
                .iter()
                .find(|(previous, observed, _)| {
                    previous
                        .as_ref()
                        .is_some_and(|task| task.state == TaskState::Queued && observed.active())
                })
                .map(|(_, _, card)| card);
            if let Some(selected_card) = selected_card {
                // The HID feed identifies cards only by AG00-AG05. Prefer the
                // authoritative Codex thread at that logical slot so its cwd,
                // title, model, and lifecycle remain attached to selection.
                // Selecting the synthetic `codex-hid:N` card would discard
                // that metadata and make display context fall back to the
                // previously active workspace.
                let task_id = selected_card
                    .source_slot
                    .and_then(|source_slot| {
                        self.tasks
                            .values()
                            .find(|task| {
                                task.owner_session == 0
                                    && task.owner_agent == AgentId::Codex
                                    && !task.task_id.starts_with("codex-hid:")
                                    && task.source_slot == Some(source_slot)
                            })
                            .map(|task| task.task_id.clone())
                    })
                    .unwrap_or_else(|| selected_card.task_id.clone());
                let device_id = self
                    .assignments
                    .get(&task_id)
                    .map(|assignment| assignment.slot.device_id.clone());
                if let Some(device_id) = device_id {
                    let _ = device_id;
                    self.selected = Some(task_id);
                    self.pending_selection = None;
                }
            }
        }

        let cards = cards
            .into_iter()
            .map(|(previous, observed, mut card)| {
                // A physical press makes the device redraw the whole board:
                // the pressed card lights up and the others fall back to
                // standby. Preserve both lifecycle and lifecycle color during
                // that echo. The frame-level handoff check also covers
                // selection performed directly in the Codex UI.
                if let Some(previous) = previous {
                    if (selection_guard_active || selection_handoff)
                        && is_selection_transition(previous.state, observed)
                    {
                        card.state = previous.state;
                        card.color = Some(previous.state.display_color());
                        card.brightness = previous.brightness;
                    }
                }
                card
            })
            .collect();
        self.merge_session_tasks(cards, now_ms);
        Ok(())
    }
    fn parse_task(
        &self,
        session: usize,
        agent: AgentId,
        item: &Value,
        now_ms: u128,
        legacy: Option<(String, String)>,
    ) -> Result<TaskCard, String> {
        let object = item
            .as_object()
            .ok_or_else(|| "each task must be an object".to_owned())?;
        let (task_id, mut legacy_key) = match legacy {
            Some((id, key)) => (id, Some(key)),
            None => {
                let id = object
                    .get("task_id")
                    .or_else(|| object.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "task_id is required".to_owned())?
                    .to_owned();
                (
                    id,
                    object
                        .get("legacy_key")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                )
            }
        };
        if task_id.is_empty() || task_id.chars().count() > 160 {
            return Err("task_id must be from 1 to 160 characters".to_owned());
        }
        if legacy_key.as_deref().is_some_and(str::is_empty) {
            legacy_key = None;
        }
        let context = object.get("context").cloned().unwrap_or(Value::Null);
        let text_field = |key: &str| {
            object
                .get(key)
                .or_else(|| context.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(160).collect::<String>())
        };
        let mut title = text_field("title")
            .or_else(|| text_field("task"))
            .or_else(|| {
                object
                    .get("t")
                    .and_then(Value::as_str)
                    .map(|value| value.chars().take(160).collect::<String>())
            })
            .unwrap_or_default();
        if legacy_key.as_deref() == Some(title.as_str()) {
            title.clear();
        }
        let project = text_field("project");
        let workspace_path = text_field("workspace_path");
        let model = text_field("model");
        let effort = text_field("effort");
        let interaction = parse_interaction(object.get("interaction"))?;
        let explicit_state = object
            .get("state")
            .or_else(|| object.get("status"))
            .and_then(Value::as_str);
        let effect = object.get("e").and_then(Value::as_u64);
        let hid_color_state = legacy_key
            .as_ref()
            .and_then(|_| object.get("color").or_else(|| object.get("c")))
            .and_then(|value| parse_color(Some(value)).ok().flatten())
            .and_then(TaskState::from_hid_color);
        // LCD effect/brightness are presentation only. In particular e:0
        // means clear the slot, never complete the task.
        let state = if legacy_key.is_some() && hid_color_state.is_some() {
            hid_color_state.expect("checked semantic HID state")
        } else if let Some(explicit_state) = explicit_state {
            TaskState::parse(Some(explicit_state))?
        } else if legacy_key.is_some() && effect == Some(1) {
            TaskState::Queued
        } else if legacy_key.is_some() && effect.unwrap_or(1) > 1 {
            TaskState::Running
        } else {
            TaskState::parse(None)?
        };
        let priority = object
            .get("priority")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .try_into()
            .map_err(|_| "priority must be from 0 to 100".to_owned())?;
        let progress = match object.get("progress") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or_else(|| "progress must be an integer".to_owned())?
                    .try_into()
                    .map_err(|_| "progress must be from 0 to 100".to_owned())?,
            ),
        };
        let color = parse_color(object.get("color").or_else(|| object.get("c")))?;
        let brightness = parse_brightness(object.get("brightness").or_else(|| object.get("b")))?;
        let source_slot = object
            .get("source_slot")
            .and_then(Value::as_u64)
            .and_then(|slot| usize::try_from(slot).ok())
            .or_else(|| {
                legacy_key
                    .as_deref()
                    .and_then(|key| key.strip_prefix("AG0"))
                    .and_then(|slot| slot.parse::<usize>().ok())
            });
        let explicit_started_at = parse_timestamp_ms(object.get("started_at_ms"))?;
        let explicit_finished_at = parse_timestamp_ms(object.get("finished_at_ms"))?;
        let timing_authoritative = object
            .get("timing_authoritative")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || object.contains_key("started_at_ms")
            || object.contains_key("finished_at_ms");
        Ok(TaskCard {
            task_id,
            owner_session: session,
            owner_agent: agent,
            title,
            project,
            workspace_path,
            model,
            effort,
            state,
            priority,
            color,
            brightness,
            progress,
            context,
            legacy_key,
            source_slot,
            timing_authoritative,
            updated_at_ms: now_ms,
            started_at_ms: explicit_started_at.or_else(|| state.active().then_some(now_ms)),
            finished_at_ms: explicit_finished_at
                .or_else(|| (state == TaskState::Completed).then_some(now_ms)),
            reconnect_until_ms: None,
            interaction,
        })
    }

    fn replace_session_tasks(&mut self, session: usize, replacement: Vec<TaskCard>, now_ms: u128) {
        let ids: HashSet<String> = replacement
            .iter()
            .map(|task| task.task_id.clone())
            .collect();
        let old_ids: Vec<String> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| {
                (task.owner_session == session && !ids.contains(id)).then_some(id.clone())
            })
            .collect();
        for id in old_ids {
            self.tasks.remove(&id);
            self.assignments.remove(&id);
            self.reviewed_completions.remove(&id);
        }
        self.merge_session_tasks(replacement, now_ms);
    }

    /// Merges partial HID status updates by logical slot. Unlike MCP task
    /// publication, a HID frame may mention only the newly changed card.
    fn merge_session_tasks(&mut self, replacement: Vec<TaskCard>, now_ms: u128) {
        self.now_ms = now_ms;
        for mut task in replacement {
            if task.state == TaskState::Completed {
                if self.reviewed_completions.contains(&task.task_id) {
                    task.state = TaskState::Queued;
                    task.color = Some(TaskState::Queued.display_color());
                    task.started_at_ms = None;
                    task.finished_at_ms = None;
                }
            } else {
                // A new lifecycle run makes a future completion reviewable
                // again, even when the source reuses the same stable task id.
                self.reviewed_completions.remove(&task.task_id);
            }
            if let Some(previous) = self.tasks.get(&task.task_id)
                && !task.timing_authoritative
            {
                // Sources without timestamps retain the legacy merge fallback.
                // Authoritative Codex records bypass this branch completely.
                let run_in_flight = previous.started_at_ms.is_some()
                    && !matches!(previous.state, TaskState::Queued | TaskState::Completed);
                task.started_at_ms = match task.state {
                    state if state.active() => {
                        if run_in_flight {
                            previous.started_at_ms
                        } else {
                            Some(now_ms)
                        }
                    }
                    TaskState::Completed => previous.started_at_ms,
                    TaskState::Waiting | TaskState::Paused | TaskState::Reconnecting => {
                        previous.started_at_ms
                    }
                    _ => None,
                };
                task.finished_at_ms = if task.state == TaskState::Completed {
                    if previous.state == TaskState::Completed {
                        previous.finished_at_ms.or(Some(now_ms))
                    } else {
                        Some(now_ms)
                    }
                } else {
                    None
                };
            }
            self.tasks.insert(task.task_id.clone(), task);
        }
        self.reallocate();
    }

    pub fn disconnect_session(&mut self, session: usize, now_ms: u128) {
        let until = now_ms + RECONNECT_GRACE.as_millis();
        for task in self
            .tasks
            .values_mut()
            .filter(|task| task.owner_session == session)
        {
            task.state = TaskState::Reconnecting;
            task.reconnect_until_ms = Some(until);
        }
        // Keep current assignments during the grace period so a transient MCP
        // reconnect does not blank the physical controls. Once the lease
        // expires, expire removes these tasks and reallocates their slots.
    }

    /// Immediately removes every task owned by `session` and reallocates the
    /// freed slots.  Unlike [`disconnect_session`], there is no grace lease:
    /// this is for synthetic owners (e.g. the Codex HID cards published from
    /// `v.oai.thstatus`) whose backing source has gone away permanently.
    pub fn clear_session(&mut self, session: usize) {
        let removed: Vec<String> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| (task.owner_session == session).then_some(id.clone()))
            .collect();
        if removed.is_empty() {
            return;
        }
        for id in &removed {
            self.tasks.remove(id);
            self.assignments.remove(id);
            self.reviewed_completions.remove(id);
        }
        self.reallocate();
    }

    /// Returns true when the board currently holds any task owned by `session`.
    pub fn has_session_tasks(&self, session: usize) -> bool {
        self.tasks
            .values()
            .any(|task| task.owner_session == session)
    }

    /// Returns true when an agent has cards from any owner other than the
    /// supplied synthetic session. Auto-feeds use this to yield immediately
    /// to an MCP client that publishes its own authoritative snapshot.
    pub fn has_agent_tasks_except(&self, agent: AgentId, excluded_session: usize) -> bool {
        self.tasks
            .values()
            .any(|task| task.owner_agent == agent && task.owner_session != excluded_session)
    }

    pub fn expire(&mut self, now_ms: u128) {
        self.now_ms = now_ms;
        let expired: Vec<String> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| {
                task.reconnect_until_ms
                    .filter(|until| *until <= now_ms)
                    .map(|_| id.clone())
            })
            .collect();
        for id in &expired {
            self.tasks.remove(id);
            self.assignments.remove(id);
            self.reviewed_completions.remove(id);
        }
        if !expired.is_empty() {
            self.reallocate();
        }
    }

    pub fn select(&mut self, device_id: &str, slot: usize, now_ms: u128) -> Option<Value> {
        let task_id = self
            .assignments
            .iter()
            .find(|(_, assignment)| {
                assignment.slot.device_id == device_id && assignment.slot.slot == slot
            })
            .map(|(id, _)| id.clone())?;
        self.select_task(&task_id, now_ms)
    }

    /// Selects an exact task identity, even if scheduler reflow moved it away
    /// from the slot where a controller last rendered it.
    pub fn select_task(&mut self, task_id: &str, now_ms: u128) -> Option<Value> {
        self.now_ms = now_ms;
        let assignment = self.assignments.get(task_id)?.slot.clone();
        let task_id = task_id.to_owned();
        let task = self.tasks.get_mut(&task_id)?;
        if task.state == TaskState::Completed {
            self.reviewed_completions.insert(task_id.clone());
            task.state = TaskState::Queued;
            task.color = Some(TaskState::Queued.display_color());
            task.started_at_ms = None;
            task.finished_at_ms = None;
        }
        let owner_agent = task.owner_agent;
        let owner_session = task.owner_session;
        let legacy_key = task.legacy_key.clone();
        self.selected = Some(task_id.clone());
        self.pending_selection = (owner_agent == AgentId::Codex)
            .then(|| (task_id.clone(), now_ms.saturating_add(2_000)));
        Some(json!({
            "type": "task_selected",
            "task_id": task_id,
            "device_id": assignment.device_id,
            "slot": assignment.slot,
            "owner_session": owner_session,
            "legacy_key": legacy_key,
            "ts": now_ms
        }))
    }

    pub fn task(&self, task_id: &str) -> Option<&TaskCard> {
        self.tasks.get(task_id)
    }

    pub fn task_at(&self, device_id: &str, slot: usize) -> Option<&TaskCard> {
        let id = self
            .assignments
            .iter()
            .find(|(_, assignment)| {
                assignment.slot.device_id == device_id && assignment.slot.slot == slot
            })
            .map(|(id, _)| id)?;
        self.tasks.get(id)
    }

    pub fn assignment(&self, task_id: &str) -> Option<&TaskAssignment> {
        self.assignments.get(task_id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &TaskCard> {
        self.tasks.values()
    }

    pub fn selected_task(&self) -> Option<&TaskCard> {
        self.selected
            .as_deref()
            .and_then(|task_id| self.tasks.get(task_id))
    }

    pub fn selected(&self, device_id: &str) -> Option<&str> {
        let task_id = self.selected.as_deref()?;
        let assignment = self.assignments.get(task_id)?;
        (assignment.slot.device_id == device_id).then_some(task_id)
    }

    pub fn selected_slot(&self, device_id: &str) -> Option<usize> {
        let task_id = self.selected(device_id)?;
        let assignment = self.assignments.get(task_id)?;
        Some(assignment.slot.slot)
    }

    pub fn selected_context(&self, device_id: &str) -> Option<&Value> {
        let task_id = self.selected(device_id)?;
        self.tasks.get(task_id).map(|task| &task.context)
    }

    pub fn selected_display_context(&self, device_id: &str) -> Option<Value> {
        let task_id = self.selected(device_id)?;
        let task = self.tasks.get(task_id)?;
        let slot = self.selected_slot(device_id)?;
        let mut context = serde_json::Map::new();
        if let Some(source) = task.context.as_object() {
            for key in [
                "project", "task", "model", "effort", "status", "progress", "task_id",
            ] {
                if let Some(value) = source.get(key) {
                    context.insert(key.to_owned(), value.clone());
                }
            }
        }
        if let Some(project) = task.project.as_ref() {
            context.insert("project".to_owned(), json!(project));
        }
        if let Some(model) = task.model.as_ref() {
            context.insert("model".to_owned(), json!(model));
        }
        if let Some(effort) = task.effort.as_ref() {
            context.insert("effort".to_owned(), json!(effort));
        }
        let task_title = context
            .get("task")
            .and_then(Value::as_str)
            .or_else(|| (!task.title.is_empty()).then_some(task.title.as_str()))
            .unwrap_or("");
        context.insert(
            "task".to_owned(),
            json!(display_task_title(slot, task_title)),
        );
        context.insert("status".to_owned(), json!(task.state.as_str()));
        if let Some(interaction) = task.interaction.as_ref() {
            context.insert("wait_reason".to_owned(), json!(interaction.kind));
            context.insert("prompt".to_owned(), json!(interaction.prompt));
            context.insert("interaction_id".to_owned(), json!(interaction.id));
            if let Some(action) = interaction.short.as_ref() {
                context.insert("short_action".to_owned(), json!(action.label));
            }
            if let Some(action) = interaction.long.as_ref() {
                context.insert("long_action".to_owned(), json!(action.label));
            }
        }
        if let Some(progress) = task.progress {
            context.insert("progress".to_owned(), json!(progress));
        }
        context
            .entry("task_id".to_owned())
            .or_insert_with(|| json!(task.task_id));
        Some(Value::Object(context))
    }

    pub fn has_tasks(&self) -> bool {
        !self.tasks.is_empty()
    }

    pub fn assignments_for_session(&self, session: usize) -> Value {
        let mut result = Vec::new();
        for task in self
            .tasks
            .values()
            .filter(|task| task.owner_session == session)
        {
            let assignment = self.assignment(&task.task_id).map(|a| {
                json!({
                    "device_id": a.slot.device_id,
                    "slot": a.slot.slot
                })
            });
            result.push(json!({"task_id": task.task_id, "assignment": assignment}));
        }
        result.sort_by(|a, b| a["task_id"].as_str().cmp(&b["task_id"].as_str()));
        json!({"tasks": result})
    }

    pub fn rendered_slots(&self, device_id: &str, slot_count: usize) -> Vec<Value> {
        (0..slot_count)
            .map(|slot| {
                let Some(task) = self.task_at(device_id, slot) else {
                    return json!({"id": slot, "e": 0, "selected": false});
                };
                let title = if task.title.is_empty() {
                    task.legacy_key
                        .clone()
                        .unwrap_or_else(|| format!("Task {}", slot + 1))
                } else {
                    task.title.clone()
                };
                // Task status updates often omit `c`; emitting zero in that
                // case suppresses the Stream Deck status palette. Queued
                // cards always retain the initial dark-grey idle background.
                let selected = self.selected(device_id) == Some(task.task_id.as_str());
                // A completed task stays green until it is selected. Selection acknowledges
                // the completion in `select`, which changes the task back to queued
                // and lets the next source snapshot retain that acknowledgement.
                let completed = task.state == TaskState::Completed;
                let color = if task.legacy_key.is_some() {
                    task.state.display_color()
                } else {
                    match task.state {
                        TaskState::Queued | TaskState::Completed | TaskState::Reconnecting => {
                            task.state.display_color()
                        }
                        _ => task.color.unwrap_or_else(|| task.state.display_color()),
                    }
                };
                json!({
                    "e": 1,
                    "id": slot,
                    "selected": selected,
                    "recently_finished": completed,
                    // Keep the semantic title separate from `t`, whose legacy
                    // fallback is an HID key label such as AG03. Stream Deck
                    // strips can then preserve a richer display-context name.
                    "title": task.title,
                    "t": title,
                    "project": task.project,
                    "workspace_path": task.workspace_path,
                    "model": task.model,
                    "effort": task.effort,
                    "status": task.state.as_str(),
                    "c": color,
                    "b": f64::from(task.brightness) / 100.0,
                    "progress": task.progress,
                    "task_id": task.task_id,
                    "agent": task.owner_agent.as_str(),
                    "started_at_ms": task.started_at_ms,
                    "finished_at_ms": task.finished_at_ms,
                    "source_slot": task.source_slot,
                    "interaction": task.interaction.as_ref().map(interaction_value)
                })
            })
            .collect()
    }
    pub fn auto_select_waiting(&mut self, device_id: &str, now_ms: u128) -> Option<Value> {
        let task_id = self.tasks.values().filter(|task| task.state == TaskState::Waiting && task.interaction.as_ref().is_some_and(|i| i.expires_at_ms.is_none_or(|expires| expires > now_ms))).filter_map(|task| { let assignment = self.assignment(&task.task_id)?; (assignment.slot.device_id == device_id).then_some(task) }).max_by(|a, b| b.priority.cmp(&a.priority).then(a.updated_at_ms.cmp(&b.updated_at_ms)).then(b.task_id.cmp(&a.task_id)))?.task_id.clone();
        if self.selected.as_deref() == Some(task_id.as_str()) { return None; }
        let assignment = self.assignment(&task_id)?.slot.clone();
        let task = self.tasks.get(&task_id)?;
        self.selected = Some(task_id.clone());
        self.pending_selection = (task.owner_agent == AgentId::Codex).then(|| (task_id.clone(), now_ms.saturating_add(2_000)));
        Some(json!({"type":"task_selected","task_id":task_id,"device_id":assignment.device_id,"slot":assignment.slot,"owner_session":task.owner_session,"legacy_key":Value::Null,"automatic":true,"ts":now_ms}))
    }

    pub fn status_json(&self) -> Value {
        let tasks = self.tasks.values().map(|task| {
            let assignment = self.assignment(&task.task_id).map(|a| json!({"device_id": a.slot.device_id, "slot": a.slot.slot}));
            json!({"task_id": task.task_id, "owner_session": task.owner_session, "owner_agent": task.owner_agent.as_str(), "title": task.title, "project": task.project, "workspace_path": task.workspace_path, "model": task.model, "effort": task.effort, "state": task.state.as_str(), "priority": task.priority, "progress": task.progress, "started_at_ms": task.started_at_ms, "finished_at_ms": task.finished_at_ms, "source_slot": task.source_slot, "assignment": assignment, "reconnect_until_ms": task.reconnect_until_ms})
        }).collect::<Vec<_>>();
        json!({"selected_task_id": self.selected, "tasks": tasks})
    }

    fn reallocate(&mut self) {
        if self
            .selected
            .as_ref()
            .is_some_and(|task_id| !self.tasks.contains_key(task_id))
        {
            self.selected = None;
            self.pending_selection = None;
        }
        let available: Vec<DeviceSlot> = self.devices.values().flatten().cloned().collect();
        let available_set: HashSet<DeviceSlot> = available.iter().cloned().collect();
        let partitioned_device_id = self.partitioned_device_id.clone();
        let slot_owners = self.slot_owners.clone();
        self.assignments.retain(|task_id, assignment| {
            self.tasks.get(task_id).is_some_and(|task| {
                task.state.eligible()
                    && available_set.contains(&assignment.slot)
                    && (partitioned_device_id.as_deref()
                        != Some(assignment.slot.device_id.as_str())
                        || slot_owners
                            .get(assignment.slot.slot)
                            .copied()
                            .is_none_or(|owner| owner == Some(task.owner_agent)))
            })
        });
        self.pin_codex_hid_primary_slots();
        // Sticky pass: an unassigned task returns to the slot it last held
        // when that slot is still free and owner-compatible. This keeps the
        // board coherent across transient evictions (repartitions, state
        // flickers, brief disconnects).
        let sticky: Vec<(String, DeviceSlot)> = self
            .tasks
            .values()
            .filter(|task| task.state.eligible() && !self.assignments.contains_key(&task.task_id))
            .filter_map(|task| {
                let slot = self.last_slots.get(&task.task_id)?;
                let free = available_set.contains(slot)
                    && !self.assignments.values().any(|a| a.slot == *slot);
                let compatible = self
                    .slot_owner(slot)
                    .map_or(true, |owner| owner == Some(task.owner_agent));
                (free && compatible).then(|| (task.task_id.clone(), slot.clone()))
            })
            .collect();
        for (task_id, slot) in sticky {
            if !self.assignments.values().any(|a| a.slot == slot) {
                self.assignments.insert(task_id, TaskAssignment { slot });
            }
        }
        let mut free: Vec<DeviceSlot> = available
            .into_iter()
            .filter(|slot| !self.assignments.values().any(|a| a.slot == *slot))
            .collect();
        free.sort_by(|a, b| {
            let a_primary = self.partitioned_device_id.as_deref() == Some(a.device_id.as_str());
            let b_primary = self.partitioned_device_id.as_deref() == Some(b.device_id.as_str());
            b_primary
                .cmp(&a_primary)
                .then(a.device_id.cmp(&b.device_id))
                .then(a.slot.cmp(&b.slot))
        });

        let mut candidates: Vec<&TaskCard> = self
            .tasks
            .values()
            .filter(|task| task.state.eligible() && !self.assignments.contains_key(&task.task_id))
            .collect();
        candidates.sort_by(|a, b| task_order(a, b));
        let mut by_session: BTreeMap<usize, Vec<&TaskCard>> = BTreeMap::new();
        for task in candidates {
            by_session.entry(task.owner_session).or_default().push(task);
        }
        let mut visible_by_session: BTreeMap<usize, usize> = BTreeMap::new();
        let mut visible_by_agent = [0usize; crate::routing::AGENT_COUNT];
        for assignment_id in self.assignments.keys() {
            if let Some(task) = self.tasks.get(assignment_id) {
                *visible_by_session.entry(task.owner_session).or_default() += 1;
                visible_by_agent[task.owner_agent.index()] += 1;
            }
        }
        for slot in free {
            let required_owner = self.slot_owner(&slot);
            let Some((&session, _)) = by_session
                .iter()
                .filter(|(_, tasks)| {
                    !tasks.is_empty()
                        && required_owner.map_or(true, |owner| owner == Some(tasks[0].owner_agent))
                })
                .min_by(|(session_a, tasks_a), (session_b, tasks_b)| {
                    visible_by_agent[tasks_a[0].owner_agent.index()]
                        .cmp(&visible_by_agent[tasks_b[0].owner_agent.index()])
                        .then(
                            visible_by_session
                                .get(session_a)
                                .unwrap_or(&0)
                                .cmp(visible_by_session.get(session_b).unwrap_or(&0)),
                        )
                        .then(task_order(tasks_a[0], tasks_b[0]))
                        .then(session_a.cmp(session_b))
                })
            else {
                // This owner has no eligible work; its slot remains reserved.
                continue;
            };
            let task = by_session
                .get_mut(&session)
                .expect("session exists")
                .remove(0);
            self.assignments
                .insert(task.task_id.clone(), TaskAssignment { slot });
            *visible_by_session.entry(session).or_default() += 1;
            visible_by_agent[task.owner_agent.index()] += 1;
        }
        for (task_id, assignment) in &self.assignments {
            self.last_slots
                .insert(task_id.clone(), assignment.slot.clone());
        }
        self.last_slots.retain(|task_id, _| {
            self.tasks.contains_key(task_id) || self.assignments.contains_key(task_id)
        });
    }

    fn slot_owner(&self, slot: &DeviceSlot) -> Option<Option<AgentId>> {
        if self.partitioned_device_id.as_deref() != Some(slot.device_id.as_str())
            || self.slot_owners.is_empty()
        {
            return None;
        }
        // The partition describes the six Micro positions. A larger primary
        // controller may have additional task-only slots; keep those slots
        // unrestricted instead of reserving them as unowned.
        self.slot_owners.get(slot.slot).copied()
    }

    /// Codex HID task IDs are logical hardware positions, not scheduler work
    /// items. Keeping each one on its matching primary slot preserves button
    /// muscle memory while other agents use their own partitioned slots.
    fn pin_codex_hid_primary_slots(&mut self) {
        let Some(primary_device_id) = self.partitioned_device_id.clone() else {
            return;
        };
        let Some(primary_slots) = self.devices.get(&primary_device_id).cloned() else {
            return;
        };
        let mut pins: Vec<(String, DeviceSlot)> = self
            .tasks
            .values()
            .filter_map(|task| {
                (task.owner_agent == AgentId::Codex)
                    .then_some(task.source_slot)
                    .flatten()
                    .and_then(|slot| {
                        // The first half remains reserved for the logical
                        // Codex HID positions while Codex is temporarily
                        // inactive. `None` must not let a ZCode refresh
                        // dislodge an already-running Codex card.
                        let owned_by_codex = self.slot_owners.is_empty()
                            || self
                                .slot_owners
                                .get(slot)
                                .copied()
                                .flatten()
                                .map_or(true, |owner| owner == AgentId::Codex);
                        owned_by_codex
                            .then(|| primary_slots.get(slot).cloned())
                            .flatten()
                    })
                    .map(|slot| (task.task_id.clone(), slot))
            })
            .collect();
        pins.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (task_id, slot) in pins {
            self.assignments.retain(|assigned_task, assignment| {
                assigned_task == &task_id || assignment.slot != slot
            });
            self.assignments.insert(task_id, TaskAssignment { slot });
        }
    }
}

fn is_selection_transition(previous: TaskState, observed: TaskState) -> bool {
    previous != observed
        && (previous == TaskState::Queued || previous.active())
        && (observed == TaskState::Queued || observed.active())
}

fn task_order(a: &TaskCard, b: &TaskCard) -> Ordering {
    a.state
        .rank()
        .cmp(&b.state.rank())
        .then(b.priority.cmp(&a.priority))
        .then(b.updated_at_ms.cmp(&a.updated_at_ms))
        .then(a.task_id.cmp(&b.task_id))
}

fn interaction_action_value(action: &TaskAction) -> Value {
    json!({"id": action.id, "label": action.label, "action": action.action, "payload": action.payload})
}

fn interaction_value(interaction: &TaskInteraction) -> Value {
    json!({"id": interaction.id, "kind": interaction.kind, "prompt": interaction.prompt, "short": interaction.short.as_ref().map(interaction_action_value), "long": interaction.long.as_ref().map(interaction_action_value), "expires_at_ms": interaction.expires_at_ms})
}

fn parse_interaction(value: Option<&Value>) -> Result<Option<TaskInteraction>, String> {
    let Some(value) = value else { return Ok(None); };
    let object = value.as_object().ok_or_else(|| "interaction must be an object".to_owned())?;
    let text = |key: &str, required: bool| -> Result<Option<String>, String> {
        match object.get(key) {
            Some(Value::String(value)) if value.chars().count() <= 160 && !value.trim().is_empty() => Ok(Some(value.clone())),
            None if !required => Ok(None),
            None => Err(format!("interaction field {key} is required")),
            Some(Value::String(_)) => Err(format!("interaction field {key} must be non-empty and at most 160 characters")),
            Some(_) => Err(format!("interaction field {key} must be a string")),
        }
    };
    let action = |key: &str| -> Result<Option<TaskAction>, String> {
        let Some(value) = object.get(key) else { return Ok(None); };
        let item = value.as_object().ok_or_else(|| format!("interaction action {key} must be an object"))?;
        let id = item.get("id").and_then(Value::as_str).ok_or_else(|| format!("interaction action {key}.id is required"))?.to_owned();
        let label = item.get("label").and_then(Value::as_str).ok_or_else(|| format!("interaction action {key}.label is required"))?.to_owned();
        let action = item.get("action").and_then(Value::as_str).ok_or_else(|| format!("interaction action {key}.action is required"))?.to_owned();
        Ok(Some(TaskAction { id, label, action, payload: item.get("payload").cloned().unwrap_or(Value::Null) }))
    };
    let id = text("id", true)?.unwrap();
    let kind = text("kind", true)?.unwrap();
    let prompt = text("prompt", true)?.unwrap();
    let expires_at_ms = parse_timestamp_ms(object.get("expires_at_ms"))?;
    let mut short = action("short")?;
    let mut long = action("long")?;
    if kind == "approval" {
        short = short.or_else(|| Some(TaskAction { id: "approve".to_owned(), label: "Approve".to_owned(), action: "approve".to_owned(), payload: Value::Null }));
        long = long.or_else(|| Some(TaskAction { id: "reject".to_owned(), label: "Reject".to_owned(), action: "reject".to_owned(), payload: Value::Null }));
    }
    Ok(Some(TaskInteraction { id, kind, prompt, short, long, expires_at_ms }))
}

fn parse_timestamp_ms(value: Option<&Value>) -> Result<Option<u128>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(u128::from)
            .map(Some)
            .ok_or_else(|| "task timestamp must be a non-negative integer".to_owned()),
        _ => Err("task timestamp must be an integer or null".to_owned()),
    }
}

fn parse_color(value: Option<&Value>) -> Result<Option<u32>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .ok_or_else(|| "color must be an integer or #RRGGBB".to_owned())
            .map(|v| Some((v & 0x00ff_ffff) as u32)),
        Some(Value::String(value)) => {
            let raw = value.strip_prefix('#').unwrap_or(value);
            if raw.len() != 6 {
                return Err("color must be #RRGGBB".to_owned());
            }
            u32::from_str_radix(raw, 16)
                .map(Some)
                .map_err(|_| "color must be #RRGGBB".to_owned())
        }
        _ => Err("color must be an integer, #RRGGBB, or null".to_owned()),
    }
}

fn parse_brightness(value: Option<&Value>) -> Result<u8, String> {
    match value {
        None | Some(Value::Null) => Ok(100),
        Some(Value::Number(value)) => {
            let value = value
                .as_f64()
                .ok_or_else(|| "brightness must be numeric".to_owned())?;
            let normalized = if value <= 1.0 { value * 100.0 } else { value };
            if !(0.0..=100.0).contains(&normalized) {
                return Err("brightness must be from 0 to 100".to_owned());
            }
            Ok(normalized.round() as u8)
        }
        _ => Err("brightness must be numeric or null".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, state: &str, priority: u64) -> Value {
        json!({"task_id": id, "title": id, "state": state, "priority": priority})
    }

    #[test]
    fn combines_devices_and_assigns_fairly() {
        let mut board = TaskBoard::new();
        board.set_device("ajazz:one", 6, true);
        board.set_device("streamdeck-plus:two", 8, true);
        board.publish_tasks(1, AgentId::Codex, &json!({"tasks": (0..8).map(|i| task(&format!("a{i}"), "running", 50)).collect::<Vec<_>>() }), 1).unwrap();
        board.publish_tasks(2, AgentId::ZCode, &json!({"tasks": (0..8).map(|i| task(&format!("b{i}"), "running", 50)).collect::<Vec<_>>() }), 2).unwrap();
        assert_eq!(board.assignments.len(), 14);
        assert!(board.tasks().any(|task| task.owner_session == 1));
        assert!(board.tasks().any(|task| task.owner_session == 2));
    }

    #[test]
    fn extended_primary_slots_accept_desktop_codex_tasks() {
        let mut board = TaskBoard::new();
        board.set_device("streamdeck", CODEX_TASK_SLOTS, true);
        board.set_slot_owners(
            "streamdeck",
            (0..8)
                .map(|slot| (slot < CODEX_HID_SLOTS).then_some(AgentId::Codex))
                .collect(),
        );
        let tasks = (0..CODEX_TASK_SLOTS)
            .map(|slot| {
                json!({
                    "task_id": format!("codex-{slot}"),
                    "title": format!("Task {slot}"),
                    "state": "queued",
                    "source_slot": slot,
                    "legacy_key": (slot < CODEX_HID_SLOTS).then(|| format!("AG0{slot}"))
                })
            })
            .collect::<Vec<_>>();
        board
            .publish_codex_snapshot(&json!({"tasks": tasks}), 1)
            .expect("publish nine Codex tasks");

        let cards = board.rendered_slots("streamdeck", CODEX_TASK_SLOTS);
        assert_eq!(cards[6]["task_id"], "codex-6");
        assert_eq!(cards[7]["task_id"], "codex-7");
        assert_eq!(cards[8]["task_id"], "codex-8");
        assert_eq!(cards[6]["legacy_key"], Value::Null);
        assert_eq!(cards[7]["legacy_key"], Value::Null);
        assert_eq!(cards[8]["legacy_key"], Value::Null);
    }
    #[test]
    fn legacy_status_accepts_i_and_percent_brightness() {
        let mut board = TaskBoard::new();
        board.set_device("plus", 8, true);
        board
            .publish_legacy_status(
                4,
                AgentId::Hermes,
                &json!([{"i":0,"e":1,"t":"hello","c":"#112233","b":80}]),
                1,
            )
            .unwrap();
        let card = board.tasks().next().unwrap();
        assert_eq!(card.color, Some(0x112233));
        assert_eq!(card.brightness, 80);
        assert_eq!(card.legacy_key.as_deref(), Some("AG00"));
    }

    #[test]
    fn legacy_status_field_sets_active_card_state_and_palette() {
        let mut board = TaskBoard::new();
        board.set_device("plus", 8, true);
        board
            .publish_legacy_status(
                4,
                AgentId::Codex,
                &json!([{"i":0,"e":1,"t":"build","status":"working"}]),
                1,
            )
            .unwrap();

        assert_eq!(board.tasks().next().unwrap().state, TaskState::Running);
        assert_eq!(board.rendered_slots("plus", 1)[0]["c"], 0x1565c0);
    }

    #[test]
    fn selected_task_provides_display_context_without_explicit_context() {
        let mut board = TaskBoard::new();
        board.set_device("plus", 8, true);
        board
            .publish_tasks(
                1,
                AgentId::Codex,
                &json!({"tasks": [{
                    "task_id": "build",
                    "title": "Build bridge",
                    "state": "running",
                    "progress": 42
                }]}),
                1,
            )
            .unwrap();
        board.select("plus", 0, 2).expect("selected task");
        let context = board.selected_display_context("plus").expect("context");
        assert_eq!(context["task"], "1 \u{2014} Build bridge");
        assert_eq!(context["status"], "running");
        assert_eq!(context["progress"], 42);
    }

    #[test]
    fn display_task_title_uses_one_based_slots_and_is_idempotent() {
        assert_eq!(
            display_task_title(0, "Build bridge"),
            "1 \u{2014} Build bridge"
        );
        assert_eq!(
            display_task_title(5, "Build bridge"),
            "6 \u{2014} Build bridge"
        );
        assert_eq!(
            display_task_title(0, "6 \u{2014} Existing label"),
            "1 \u{2014} Existing label"
        );
        assert_eq!(display_task_title(6, "Build bridge"), "7 \u{2014} Build bridge");
    }

    #[test]
    fn selected_display_context_prefixes_explicit_task_title_without_changing_task_id() {
        let mut board = TaskBoard::new();
        board.set_device("plus", 6, true);
        board
            .publish_tasks(
                1,
                AgentId::Codex,
                &json!({"tasks": (0..6).map(|i| json!({
                    "task_id": format!("task-{i}"),
                    "title": format!("Task {i}"),
                    "state": "running",
                    "context": {"task": format!("Live task {i}"), "task_id": format!("wire-{i}")}
                })).collect::<Vec<_>>() }),
                1,
            )
            .unwrap();
        board.select("plus", 5, 2).expect("selected task");

        let context = board.selected_display_context("plus").expect("context");
        assert_eq!(context["task"], "6 \u{2014} Live task 5");
        assert_eq!(context["task_id"], "wire-5");
    }

    #[test]
    fn reconnect_lease_expires() {
        let mut board = TaskBoard::new();
        board.set_device("plus", 8, true);
        board
            .publish_tasks(
                1,
                AgentId::Codex,
                &json!({"tasks":[task("x", "running", 50)]}),
                1,
            )
            .unwrap();
        board.disconnect_session(1, 2);
        assert_eq!(board.tasks().next().unwrap().state, TaskState::Reconnecting);
        assert_eq!(board.assignment("x").unwrap().slot.slot, 0);
        assert_eq!(board.rendered_slots("plus", 1)[0]["e"], 1);
        board.expire(2 + RECONNECT_GRACE.as_millis());
        assert_eq!(board.tasks().count(), 0);
    }

    #[test]
    fn partitioned_primary_slots_reserve_idle_agents_and_reflow() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board.publish_tasks(
            1,
            AgentId::Codex,
            &json!({"tasks": (0..6).map(|i| task(&format!("c{i}"), "running", 50)).collect::<Vec<_>>() }),
            1,
        ).unwrap();

        board.set_slot_owners(
            "primary",
            vec![
                Some(AgentId::Codex),
                Some(AgentId::ZCode),
                Some(AgentId::Hermes),
                Some(AgentId::Codex),
                Some(AgentId::ZCode),
                Some(AgentId::Hermes),
            ],
        );
        board
            .publish_tasks(
                2,
                AgentId::ZCode,
                &json!({"tasks": [task("z", "running", 50)]}),
                2,
            )
            .unwrap();

        let cards = board.rendered_slots("primary", 6);
        assert_eq!(cards[0]["agent"], "codex");
        assert_eq!(cards[3]["agent"], "codex");
        assert_eq!(cards[1]["agent"], "zcode");
        assert_eq!(cards[2]["e"], 0);
        assert_eq!(cards[4]["e"], 0);
        assert_eq!(cards[5]["e"], 0);

        board.set_slot_owners(
            "primary",
            vec![
                Some(AgentId::Codex),
                Some(AgentId::Codex),
                Some(AgentId::Codex),
                Some(AgentId::Hermes),
                Some(AgentId::Hermes),
                Some(AgentId::Hermes),
            ],
        );
        let cards = board.rendered_slots("primary", 6);
        assert!(cards[..3].iter().all(|card| card["agent"] == "codex"));
        assert!(cards[3..].iter().all(|card| card["e"] == 0));
    }

    #[test]
    fn repartition_with_unchanged_ownership_keeps_cards_in_place() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board.set_slot_owners("primary", vec![Some(AgentId::Codex); 6]);
        board
            .publish_tasks(
                1,
                AgentId::Codex,
                &json!({"tasks": (0..4).map(|i| task(&format!("c{i}"), "running", 50)).collect::<Vec<_>>() }),
                1,
            )
            .unwrap();
        let before: Vec<Option<usize>> = (0..4)
            .map(|i| board.assignment(&format!("c{i}")).map(|a| a.slot.slot))
            .collect();

        // A repartition that still grants Codex the occupied slots (e.g.
        // triggered by a task-button press waking the serial link) must not
        // shuffle the cards that already comply with the new owner map.
        board.set_slot_owners(
            "primary",
            vec![
                Some(AgentId::Codex),
                Some(AgentId::Codex),
                Some(AgentId::Codex),
                Some(AgentId::Codex),
                Some(AgentId::ZCode),
                Some(AgentId::ZCode),
            ],
        );
        let after: Vec<Option<usize>> = (0..4)
            .map(|i| board.assignment(&format!("c{i}")).map(|a| a.slot.slot))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn evicted_task_returns_to_its_previous_slot() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_tasks(
                1,
                AgentId::Codex,
                &json!({"tasks": [task("a", "running", 50), task("b", "running", 50)]}),
                1,
            )
            .unwrap();
        let slot_b = board.assignment("b").expect("b assigned").slot.slot;

        // Drop "b" and republish it: it comes back on the same slot instead
        // of shuffling the board.
        board
            .publish_tasks(
                1,
                AgentId::Codex,
                &json!({"tasks": [task("a", "running", 50)]}),
                2,
            )
            .unwrap();
        board
            .publish_tasks(
                1,
                AgentId::Codex,
                &json!({"tasks": [task("a", "running", 50), task("b", "running", 50)]}),
                3,
            )
            .unwrap();
        assert_eq!(
            board.assignment("b").expect("b reassigned").slot.slot,
            slot_b
        );
    }

    #[test]
    fn partitioned_primary_is_scheduled_before_auxiliary_devices() {
        let mut board = TaskBoard::new();
        board.set_device("aaa-aux", 6, true);
        board.set_device("primary", 6, true);
        board.set_slot_owners(
            "primary",
            vec![
                None,
                None,
                None,
                Some(AgentId::ZCode),
                Some(AgentId::ZCode),
                Some(AgentId::ZCode),
            ],
        );

        board
            .publish_tasks(
                1,
                AgentId::ZCode,
                &json!({"tasks": [task("z", "running", 50)]}),
                1,
            )
            .unwrap();

        let assignment = board.assignment("z").expect("assignment");
        assert_eq!(assignment.slot.device_id, "primary");
        assert_eq!(assignment.slot.slot, 3);
    }
    #[test]
    fn partition_capacity_is_shared_without_dropping_session_tasks() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board.set_slot_owners(
            "primary",
            vec![
                Some(AgentId::Codex),
                Some(AgentId::Codex),
                Some(AgentId::Codex),
                Some(AgentId::Hermes),
                Some(AgentId::Hermes),
                Some(AgentId::Hermes),
            ],
        );
        for session in [1, 2] {
            board.publish_tasks(
                session,
                AgentId::Codex,
                &json!({"tasks": (0..3).map(|i| task(&format!("c{session}-{i}"), "running", 50)).collect::<Vec<_>>() }),
                session as u128,
            ).unwrap();
        }

        assert_eq!(
            board
                .tasks()
                .filter(|task| task.owner_agent == AgentId::Codex)
                .count(),
            6
        );
        assert_eq!(
            board
                .tasks()
                .filter(|task| task.owner_agent == AgentId::Codex
                    && board.assignment(&task.task_id).is_some())
                .count(),
            3
        );
        assert!(
            board
                .tasks()
                .filter(|task| task.owner_agent == AgentId::Codex)
                .filter_map(|task| board.assignment(&task.task_id))
                .all(|assignment| assignment.slot.slot < 3)
        );
    }

    #[test]
    fn publish_before_repartition_is_retained_and_becomes_visible() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board.set_slot_owners("primary", vec![None; 6]);
        board
            .publish_tasks(
                7,
                AgentId::Hermes,
                &json!({"tasks": [task("early", "running", 50)]}),
                1,
            )
            .unwrap();

        assert_eq!(board.tasks().count(), 1);
        assert!(board.assignment("early").is_none());

        board.set_slot_owners("primary", vec![Some(AgentId::Hermes); 6]);

        assert_eq!(board.rendered_slots("primary", 6)[0]["task_id"], "early");
    }

    #[test]
    fn codex_hid_tracks_concurrent_running_idle_and_completed_slots() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 2, "status": "running"},
                    {"id": 1, "e": 2, "status": "running"}
                ]),
                10,
            )
            .unwrap();

        assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Running);
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 1},
                    {"id": 1, "e": 2, "status": "running"}
                ]),
                20,
            )
            .unwrap();

        assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Queued);
        assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:0"].started_at_ms, None);
        assert_eq!(board.tasks["codex-hid:1"].started_at_ms, Some(10));

        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 1},
                    {"id": 1, "e": 0, "status": "running", "c": 0x00ff00}
                ]),
                30,
            )
            .unwrap();

        assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Queued);
        assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:1"].started_at_ms, Some(10));
        assert_eq!(board.tasks["codex-hid:1"].finished_at_ms, None);
    }

    #[test]
    fn starting_another_hid_task_keeps_blue_background_tasks_running() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 2, "c": 0x1565c0},
                    {"id": 1, "e": 1, "c": 0x37474f}
                ]),
                10,
            )
            .unwrap();
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 1, "c": 0x1565c0},
                    {"id": 1, "e": 2, "c": 0x1565c0}
                ]),
                20,
            )
            .unwrap();

        assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:0"].started_at_ms, Some(10));
        assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:1"].started_at_ms, Some(20));
    }

    #[test]
    fn solid_green_hid_card_completes_even_with_stale_running_status() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 1, true);
        board
            .publish_codex_hid_status(
                &json!([{"id": 0, "e": 2, "c": 0x1565c0, "status": "running"}]),
                10,
            )
            .unwrap();
        board
            .publish_codex_hid_status(
                &json!([{"id": 0, "e": 1, "c": 0x2e7d32, "status": "running"}]),
                30,
            )
            .unwrap();

        let task = &board.tasks["codex-hid:0"];
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(task.started_at_ms, Some(10));
        assert_eq!(task.finished_at_ms, Some(30));
        // Completion remains green until the card is selected.
        assert_eq!(board.rendered_slots("primary", 1)[0]["c"], 0x1b5e20);
    }

    #[test]
    fn zero_brightness_only_clears_presentation_without_finishing_task() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 1, true);
        board
            .publish_codex_hid_status(&json!([{"id": 0, "e": 2, "b": 1, "c": 0x1565c0}]), 10)
            .unwrap();
        board
            .publish_codex_hid_status(&json!([{"id": 0, "e": 1, "b": 0, "c": 0x1565c0}]), 30)
            .unwrap();

        let task = &board.tasks["codex-hid:0"];
        assert_eq!(task.state, TaskState::Running);
        assert_eq!(task.started_at_ms, Some(10));
        assert_eq!(task.finished_at_ms, None);
    }

    #[test]
    fn new_task_redraw_does_not_complete_zero_brightness_running_task() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 2, true);
        board
            .publish_codex_hid_status(&json!([{"id": 0, "e": 2, "b": 1, "c": 0x1565c0}]), 10)
            .unwrap();
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 1, "b": 0, "c": 0x1565c0},
                    {"id": 1, "e": 2, "b": 1, "c": 0x1565c0}
                ]),
                20,
            )
            .unwrap();

        assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:0"].started_at_ms, Some(10));
        assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:1"].started_at_ms, Some(20));
    }
    #[test]
    fn task_timer_resets_for_each_new_running_iteration() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 1, true);
        for (state, now) in [
            ("queued", 10),
            ("running", 20),
            ("queued", 30),
            ("running", 40),
        ] {
            board
                .publish_tasks(
                    1,
                    AgentId::Codex,
                    &json!({"tasks": [task("loop", state, 50)]}),
                    now,
                )
                .unwrap();
        }

        assert_eq!(board.tasks["loop"].started_at_ms, Some(40));
        assert_eq!(board.tasks["loop"].finished_at_ms, None);
        board
            .publish_tasks(
                1,
                AgentId::Codex,
                &json!({"tasks": [task("loop", "completed", 50)]}),
                50,
            )
            .unwrap();
        assert_eq!(board.tasks["loop"].started_at_ms, Some(40));
        assert_eq!(board.tasks["loop"].finished_at_ms, Some(50));
    }
    #[test]
    fn codex_button_selection_does_not_start_idle_task_timer() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 2, "status": "running"},
                    {"id": 1, "e": 1}
                ]),
                10,
            )
            .unwrap();

        board.guard_selection_activation("codex-hid:1", 20);
        // The device may echo the selection redraw over several frames; every
        // cosmetic flip within the guard window is absorbed.
        for echo_at in [21, 22] {
            board
                .publish_codex_hid_status(
                    &json!([
                        {"id": 0, "e": 1},
                        {"id": 1, "e": 2, "status": "running"}
                    ]),
                    echo_at,
                )
                .unwrap();
            assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Running);
            assert_eq!(board.tasks["codex-hid:0"].started_at_ms, Some(10));
            assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Queued);
            assert_eq!(board.tasks["codex-hid:1"].started_at_ms, None);
        }

        // A task-only update after the guard window is genuine activity. It
        // starts the selected task without demoting the already-running task.
        board
            .publish_codex_hid_status(&json!([{"id": 1, "e": 2, "status": "running"}]), 1_100)
            .unwrap();
        assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:0"].started_at_ms, Some(10));
        assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:1"].started_at_ms, Some(1_100));
    }

    #[test]
    fn codex_ui_selection_handoff_preserves_lifecycle_state_and_color() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 2, "b": 1, "c": 0x1565c0},
                    {"id": 1, "e": 1, "b": 0.7, "c": 0x37474f}
                ]),
                10,
            )
            .unwrap();
        board.select("primary", 0, 11).expect("select running task");
        let initial_cards = board.rendered_slots("primary", 6);
        assert_eq!(initial_cards[0]["status"], "running");
        assert_eq!(initial_cards[0]["selected"], true);
        assert_eq!(initial_cards[1]["status"], "queued");
        assert_eq!(initial_cards[1]["selected"], false);

        // Selecting the existing idle task in Codex swaps the presentation
        // colors/effects across the complete board without a bridge button
        // event. Selection must not become task execution.
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 1, "b": 0.7, "c": 0x37474f},
                    {"id": 1, "e": 2, "b": 1, "c": 0x1565c0}
                ]),
                2_000,
            )
            .unwrap();

        assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:0"].started_at_ms, Some(10));
        assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Queued);
        assert_eq!(board.tasks["codex-hid:1"].started_at_ms, None);

        let cards = board.rendered_slots("primary", 6);
        assert_eq!(cards[0]["status"], "running");
        assert_eq!(cards[0]["c"], 0x1565c0);
        assert_eq!(cards[0]["selected"], false);
        assert_eq!(cards[1]["status"], "queued");
        assert_eq!(cards[1]["c"], 0x37474f);
        assert_eq!(cards[1]["selected"], true);
    }

    #[test]
    fn codex_hid_selection_handoff_keeps_authoritative_cross_project_identity() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_codex_snapshot(
                &json!({
                    "selected_task_id": "thread-a",
                    "tasks": [
                        {
                            "task_id": "thread-a",
                            "title": "First project task",
                            "project": "first-project",
                            "state": "running",
                            "source_slot": 0,
                            "legacy_key": "AG00"
                        },
                        {
                            "task_id": "thread-b",
                            "title": "Other project task",
                            "project": "other-project",
                            "state": "queued",
                            "source_slot": 1,
                            "legacy_key": "AG01"
                        }
                    ]
                }),
                10,
            )
            .expect("Codex snapshot");
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 2, "c": 0x1565c0},
                    {"id": 1, "e": 1, "c": 0x37474f}
                ]),
                20,
            )
            .expect("initial HID state");

        // Selecting the task from the other project swaps only HID
        // presentation state. The bridge must retain the real thread card,
        // which is the source of authoritative project metadata.
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 1, "c": 0x37474f},
                    {"id": 1, "e": 2, "c": 0x1565c0}
                ]),
                30,
            )
            .expect("selection handoff");

        assert_eq!(board.selected("primary"), Some("thread-b"));
        let context = board
            .selected_display_context("primary")
            .expect("selected context");
        assert_eq!(context["task_id"], "thread-b");
        assert_eq!(context["project"], "other-project");
        assert_eq!(context["task"], display_task_title(1, "Other project task"));
    }

    #[test]
    fn partial_codex_hid_update_keeps_other_running_cards() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 2, "status": "running"},
                    {"id": 1, "e": 2, "status": "running"}
                ]),
                10,
            )
            .unwrap();

        board
            .publish_codex_hid_status(&json!([{"id": 2, "e": 2, "status": "running"}]), 20)
            .unwrap();

        assert_eq!(board.tasks["codex-hid:0"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:0"].started_at_ms, Some(10));
        assert_eq!(board.tasks["codex-hid:1"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:1"].started_at_ms, Some(10));
        assert_eq!(board.tasks["codex-hid:2"].state, TaskState::Running);
        assert_eq!(board.tasks["codex-hid:2"].started_at_ms, Some(20));
    }

    #[test]
    fn codex_hid_cards_keep_their_matching_primary_slots() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board.set_slot_owners("primary", vec![Some(AgentId::Codex); 6]);
        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 2, "e": 2, "status": "running"},
                    {"id": 0, "e": 2, "status": "running"},
                    {"id": 1, "e": 1}
                ]),
                10,
            )
            .unwrap();

        for slot in 0..3 {
            assert_eq!(
                board
                    .assignment(&format!("codex-hid:{slot}"))
                    .unwrap()
                    .slot
                    .slot,
                slot
            );
        }

        board
            .publish_codex_hid_status(
                &json!([
                    {"id": 0, "e": 1},
                    {"id": 1, "e": 2, "status": "running"},
                    {"id": 2, "e": 2, "status": "thinking"}
                ]),
                20,
            )
            .unwrap();
        for slot in 0..3 {
            assert_eq!(
                board
                    .assignment(&format!("codex-hid:{slot}"))
                    .unwrap()
                    .slot
                    .slot,
                slot
            );
        }
    }

    #[test]
    fn zcode_partition_does_not_displace_existing_codex_hid_cards() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board.set_slot_owners(
            "primary",
            vec![
                None,
                None,
                None,
                Some(AgentId::ZCode),
                Some(AgentId::ZCode),
                Some(AgentId::ZCode),
            ],
        );
        board
            .publish_codex_hid_status(&json!([{"id": 0, "e": 2, "status": "running"}]), 10)
            .unwrap();
        board
            .publish_tasks(
                7,
                AgentId::ZCode,
                &json!({"tasks": [task("z", "running", 50)]}),
                20,
            )
            .unwrap();

        assert_eq!(board.assignment("codex-hid:0").unwrap().slot.slot, 0);
        assert_eq!(board.assignment("z").unwrap().slot.slot, 3);
    }

    #[test]
    fn thinking_status_is_preserved() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_codex_hid_status(
                &json!([{"id": 0, "e": 2, "status": "thinking", "t": "Reasoning"}]),
                10,
            )
            .unwrap();

        let cards = board.rendered_slots("primary", 6);
        assert_eq!(cards[0]["status"], "thinking");
        assert_eq!(cards[0]["c"], 0x6a1b9a);
        assert_eq!(cards[0]["title"], "Reasoning");
        assert_eq!(cards[0]["t"], "Reasoning");
    }

    #[test]
    fn rendered_hid_slot_separates_task_title_from_legacy_key_label() {
        let mut board = TaskBoard::new();
        board.set_device("primary", 6, true);
        board
            .publish_codex_hid_status(&json!([{"id": 3, "e": 1, "status": "idle"}]), 10)
            .unwrap();

        let cards = board.rendered_slots("primary", 6);
        let card = cards
            .iter()
            .find(|card| card["task_id"] == "codex-hid:3")
            .expect("rendered HID card");
        assert_eq!(card["title"], "");
        assert_eq!(card["t"], "AG03");
    }
    #[test]
    fn clear_session_frees_slots_for_another_owner() {
        let mut board = TaskBoard::new();
        board.set_device("plus", 6, true);
        // Synthetic Codex HID cards (session 0) hog every slot, mirroring the
        // stuck board seen when the RP2040 is offline.
        board
            .publish_tasks(
                0,
                AgentId::Codex,
                &json!({"tasks": (0..6).map(|i| task(&format!("codex-hid:{i}"), "queued", 50)).collect::<Vec<_>>() }),
                1,
            )
            .unwrap();
        assert!(board.has_session_tasks(0));
        assert_eq!(board.assignment("codex-hid:0").unwrap().slot.slot, 0);

        // Clearing session 0 immediately releases the slots.
        board.clear_session(0);
        assert!(!board.has_session_tasks(0));

        // A subsequent ZCode publish reclaims slot 0.
        board
            .publish_tasks(
                7,
                AgentId::ZCode,
                &json!({"tasks":[task("zcode:sess_1", "running", 75)]}),
                2,
            )
            .unwrap();
        assert_eq!(board.assignment("zcode:sess_1").unwrap().slot.slot, 0);
    }

    #[test]
    fn codex_snapshot_embeds_metadata_lifecycle_and_exact_timestamps() {
        let mut board = TaskBoard::new();
        board.set_device("deck", 6, true);
        board.set_slot_owners("deck", vec![Some(AgentId::Codex); 6]);
        board
            .publish_codex_snapshot(
                &json!({
                    "selected_task_id": "thread-b",
                    "tasks": [
                        {
                            "task_id": "thread-a",
                            "title": "Running A",
                            "project": "micro-emu",
                            "model": "gpt-a",
                            "effort": "high",
                            "state": "running",
                            "source_slot": 0,
                            "legacy_key": "AG00",
                            "started_at_ms": 1000,
                            "finished_at_ms": null,
                            "timing_authoritative": true
                        },
                        {
                            "task_id": "thread-b",
                            "title": "Finished B",
                            "project": "bridge",
                            "model": "gpt-b",
                            "effort": "medium",
                            "state": "completed",
                            "source_slot": 1,
                            "legacy_key": "AG01",
                            "started_at_ms": 2000,
                            "finished_at_ms": 5000,
                            "timing_authoritative": true
                        }
                    ]
                }),
                10_000,
            )
            .expect("Codex snapshot");

        let cards = board.rendered_slots("deck", 6);
        assert_eq!(
            cards.iter().filter(|card| card["selected"] == true).count(),
            1
        );
        assert_eq!(cards[1]["selected"], true);
        assert_eq!(cards[1]["task_id"], "thread-b");
        assert_eq!(cards[1]["title"], "Finished B");
        assert_eq!(cards[1]["project"], "bridge");
        assert_eq!(cards[1]["model"], "gpt-b");
        assert_eq!(cards[1]["effort"], "medium");
        assert_eq!(cards[1]["status"], "completed");
        assert_eq!(cards[1]["e"], 1);
        assert_eq!(cards[1]["started_at_ms"], 2000);
        assert_eq!(cards[1]["finished_at_ms"], 5000);

        let context = board
            .selected_display_context("deck")
            .expect("selected context");
        assert_eq!(context["task"], display_task_title(1, "Finished B"));
        assert_eq!(context["project"], "bridge");
        assert_eq!(context["model"], "gpt-b");
        assert_eq!(context["effort"], "medium");
        assert_eq!(context["task_id"], "thread-b");
    }

    #[test]
    fn codex_snapshot_timers_never_reset_during_refresh() {
        let mut board = TaskBoard::new();
        board.set_device("deck", 1, true);
        let running = json!({
            "selected_task_id": "thread",
            "tasks": [{
                "task_id": "thread",
                "title": "Stable timer",
                "state": "running",
                "source_slot": 0,
                "legacy_key": "AG00",
                "started_at_ms": 123_000,
                "finished_at_ms": null,
                "timing_authoritative": true
            }]
        });
        board
            .publish_codex_snapshot(&running, 200_000)
            .expect("first snapshot");
        board
            .publish_codex_snapshot(&running, 900_000)
            .expect("refresh snapshot");
        assert_eq!(board.tasks["thread"].started_at_ms, Some(123_000));
        assert_eq!(board.tasks["thread"].finished_at_ms, None);

        board
            .publish_codex_snapshot(
                &json!({
                    "selected_task_id": "thread",
                    "tasks": [{
                        "task_id": "thread",
                        "title": "Stable timer",
                        "state": "completed",
                        "source_slot": 0,
                        "legacy_key": "AG00",
                        "started_at_ms": 123_000,
                        "finished_at_ms": 456_000,
                        "timing_authoritative": true
                    }]
                }),
                1_000_000,
            )
            .expect("completed snapshot");
        assert_eq!(board.tasks["thread"].started_at_ms, Some(123_000));
        assert_eq!(board.tasks["thread"].finished_at_ms, Some(456_000));
    }

    #[test]
    fn zcode_completion_stays_green_until_selection() {
        let mut board = TaskBoard::new();
        board.set_device("deck", 1, true);
        board
            .publish_tasks(
                999,
                AgentId::ZCode,
                &json!({"tasks":[{"task_id":"z","title":"ZCode result","state":"completed","started_at_ms":100,"finished_at_ms":200,"timing_authoritative":true}]}),
                300,
            )
            .unwrap();

        let cards = board.rendered_slots("deck", 1);
        assert_eq!(cards[0]["selected"], false);
        assert_eq!(cards[0]["recently_finished"], true);
        assert_eq!(cards[0]["c"], 0x1b5e20);

        board.select("deck", 0, 30_201).expect("select completed task");
        let reviewed = board.rendered_slots("deck", 1);
        assert_eq!(reviewed[0]["status"], "queued");
        assert_eq!(reviewed[0]["c"], 0x37474f);
    }

    #[test]
    fn hermes_completion_stays_green_until_selection() {
        let mut board = TaskBoard::new();
        board.set_device("deck", 1, true);
        board
            .publish_tasks(
                998,
                AgentId::Hermes,
                &json!({"tasks":[{"task_id":"h","title":"Hermes result","state":"completed","started_at_ms":100,"finished_at_ms":200,"timing_authoritative":true}]}),
                300,
            )
            .unwrap();

        let cards = board.rendered_slots("deck", 1);
        assert_eq!(cards[0]["selected"], false);
        assert_eq!(cards[0]["recently_finished"], true);
        assert_eq!(cards[0]["c"], 0x1b5e20);

        board.select("deck", 0, 30_201).expect("select completed task");
        let reviewed = board.rendered_slots("deck", 1);
        assert_eq!(reviewed[0]["status"], "queued");
        assert_eq!(reviewed[0]["c"], 0x37474f);
    }

    #[test]
    fn completion_green_persists_until_selected() {
        let mut board = TaskBoard::new();
        board.set_device("deck", 2, true);
        board.set_slot_owners("deck", vec![Some(AgentId::Codex); 2]);
        board
            .publish_codex_snapshot(
                &json!({
                    "selected_task_id": "recent",
                    "tasks": [
                        {
                            "task_id": "recent",
                            "title": "Recent result",
                            "state": "completed",
                            "source_slot": 0,
                            "legacy_key": "AG00",
                            "started_at_ms": 100,
                            "finished_at_ms": 200,
                            "timing_authoritative": true
                        },
                        {
                            "task_id": "older",
                            "title": "Older result",
                            "state": "completed",
                            "source_slot": 1,
                            "legacy_key": "AG01",
                            "started_at_ms": 50,
                            "finished_at_ms": 150,
                            "timing_authoritative": true
                        }
                    ]
                }),
                300,
            )
            .expect("completed snapshot");

        let cards = board.rendered_slots("deck", 2);
        assert_eq!(cards[0]["selected"], true);
        assert_eq!(cards[0]["recently_finished"], true);
        assert_eq!(cards[0]["c"], 0x1b5e20);
        assert_eq!(cards[1]["selected"], false);
        assert_eq!(cards[1]["recently_finished"], true);
        assert_eq!(cards[1]["c"], 0x1b5e20);

        // Time does not clear an unselected completion. Selecting the card
        // acknowledges it and returns it to the idle palette.
        board.expire(30_201);
        let persistent = board.rendered_slots("deck", 2);
        assert_eq!(persistent[0]["c"], 0x1b5e20);
        assert_eq!(persistent[1]["c"], 0x1b5e20);
        board.select("deck", 0, 30_202).expect("select completed task");
        let reviewed = board.rendered_slots("deck", 2);
        assert_eq!(reviewed[0]["status"], "queued");
        assert_eq!(reviewed[0]["c"], 0x37474f);
    }

    #[test]
    fn selecting_completed_task_marks_it_idle_until_lifecycle_restarts() {
        let mut board = TaskBoard::new();
        board.set_device("deck", 1, true);
        let snapshot = |state: &str| {
            json!({
                "selected_task_id": "thread",
                "tasks": [{
                    "task_id": "thread",
                    "title": "Review me",
                    "state": state,
                    "source_slot": 0,
                    "legacy_key": "AG00",
                    "started_at_ms": 100,
                    "finished_at_ms": if state == "completed" { Some(200) } else { None },
                    "timing_authoritative": true
                }]
            })
        };

        board
            .publish_codex_snapshot(&snapshot("completed"), 300)
            .expect("completed snapshot");
        assert_eq!(board.rendered_slots("deck", 1)[0]["status"], "completed");
        assert_eq!(board.rendered_slots("deck", 1)[0]["c"], 0x1b5e20);

        let event = board.select("deck", 0, 400).expect("select completed task");
        assert_eq!(event["type"], "task_selected");
        let reviewed = &board.rendered_slots("deck", 1)[0];
        assert_eq!(reviewed["status"], "queued");
        assert_eq!(reviewed["c"], 0x37474f);
        assert_eq!(reviewed["started_at_ms"], Value::Null);
        assert_eq!(reviewed["finished_at_ms"], Value::Null);

        // The desktop snapshot remains completed after review. It must not
        // repaint the acknowledged card green on the next polling tick.
        board
            .publish_codex_snapshot(&snapshot("completed"), 500)
            .expect("repeated completed snapshot");
        assert_eq!(board.rendered_slots("deck", 1)[0]["status"], "queued");

        // Once the same task starts a new lifecycle, its next completion is
        // unreviewed and therefore becomes green again.
        board
            .publish_codex_snapshot(&snapshot("running"), 600)
            .expect("new running lifecycle");
        assert_eq!(board.rendered_slots("deck", 1)[0]["status"], "running");
        board
            .publish_codex_snapshot(&snapshot("completed"), 700)
            .expect("new completion");
        assert_eq!(board.rendered_slots("deck", 1)[0]["status"], "completed");
        assert_eq!(board.rendered_slots("deck", 1)[0]["c"], 0x1b5e20);
    }

    #[test]
    fn selection_is_global_and_stale_focus_cannot_override_a_pending_press() {
        let mut board = TaskBoard::new();
        board.set_device("deck", 2, true);
        let snapshot = |selected: &str| {
            json!({
                "selected_task_id": selected,
                "tasks": [
                    {"task_id":"a","title":"A","state":"queued","source_slot":0,"legacy_key":"AG00"},
                    {"task_id":"b","title":"B","state":"queued","source_slot":1,"legacy_key":"AG01"}
                ]
            })
        };
        board
            .publish_codex_snapshot(&snapshot("a"), 100)
            .expect("initial snapshot");
        board.select("deck", 1, 200).expect("press B");
        board
            .publish_codex_snapshot(&snapshot("a"), 250)
            .expect("stale focus snapshot");
        assert_eq!(board.selected("deck"), Some("b"));
        assert_eq!(
            board
                .rendered_slots("deck", 2)
                .iter()
                .filter(|card| card["selected"] == true)
                .count(),
            1
        );

        board
            .publish_codex_snapshot(&snapshot("b"), 300)
            .expect("confirmed focus snapshot");
        assert_eq!(board.selected("deck"), Some("b"));

        board.set_device("deck", 0, false);
        board.set_device("reconnected", 2, true);
        assert_eq!(board.selected("reconnected"), Some("b"));
        assert_eq!(
            board
                .rendered_slots("reconnected", 2)
                .iter()
                .filter(|card| card["selected"] == true)
                .count(),
            1
        );
    }
    #[test]
    fn approval_interaction_defaults_and_auto_selects_waiting_task() {
        let mut board = TaskBoard::new();
        board.set_device("deck", 2, true);
        board.publish_tasks(7, AgentId::Hermes, &json!({"tasks":[{"task_id":"approval","title":"Deploy","state":"waiting","priority":90,"interaction":{"id":"ask-1","kind":"approval","prompt":"Deploy now?"}}]}), 100).unwrap();
        let selection = board.auto_select_waiting("deck", 100).expect("automatic selection");
        assert_eq!(selection["task_id"], "approval");
        let rendered = board.rendered_slots("deck", 2);
        assert_eq!(rendered[0]["interaction"]["short"]["action"], "approve");
        assert_eq!(rendered[0]["interaction"]["long"]["action"], "reject");
        let context = board.selected_display_context("deck").unwrap();
        assert_eq!(context["prompt"], "Deploy now?");
    }
}
