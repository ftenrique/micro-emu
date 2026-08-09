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
use std::time::Duration;

pub const RECONNECT_GRACE: Duration = Duration::from_secs(30);
pub const CODEX_TASK_SLOTS: usize = 6;

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
            Self::Running => 1,
            Self::Queued | Self::Paused => 2,
            Self::Completed => 3,
            Self::Reconnecting => 4,
        }
    }

    fn eligible(self) -> bool {
        !matches!(self, Self::Reconnecting)
    }

    fn display_color(self) -> u32 {
        match self {
            Self::Queued | Self::Reconnecting => 0x37474f,
            Self::Running => 0x1565c0,
            Self::Waiting | Self::Paused => 0xef6c00,
            Self::Error => 0xb71c1c,
            Self::Completed => 0x2e7d32,
        }
    }

}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskCard {
    pub task_id: String,
    pub owner_session: usize,
    pub owner_agent: AgentId,
    pub title: String,
    pub state: TaskState,
    pub priority: u8,
    pub color: Option<u32>,
    pub brightness: u8,
    pub progress: Option<u8>,
    pub context: Value,
    pub legacy_key: Option<String>,
    pub updated_at_ms: u128,
    pub reconnect_until_ms: Option<u128>,
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
    selected: HashMap<String, String>,
}

impl TaskBoard {
    pub fn new() -> Self {
        Self::default()
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
            self.selected.remove(&device_id);
        }
        self.reallocate();
    }

    pub fn clear_devices(&mut self) {
        self.devices.clear();
        self.assignments.clear();
        self.selected.clear();
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
        self.replace_session_tasks(session, replacement, now_ms);
        Ok(self.assignments_for_session(session))
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
        let session = 0;
        let mut cards = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate().take(6) {
            if entry.get("e").and_then(Value::as_u64) == Some(0) {
                continue;
            }
            cards.push(self.parse_task(
                session,
                AgentId::Codex,
                entry,
                now_ms,
                Some((format!("codex-hid:{index}"), format!("AG0{index}"))),
            )?);
        }
        self.replace_session_tasks(session, cards, now_ms);
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
        let (task_id, legacy_key) = match legacy {
            Some((id, key)) => (id, Some(key)),
            None => {
                let id = object
                    .get("task_id")
                    .or_else(|| object.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "task_id is required".to_owned())?
                    .to_owned();
                (id, None)
            }
        };
        if task_id.is_empty() || task_id.chars().count() > 160 {
            return Err("task_id must be from 1 to 160 characters".to_owned());
        }
        let title = object
            .get("title")
            .or_else(|| object.get("t"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(160)
            .collect::<String>();
        let state = TaskState::parse(
            object
                .get("state")
                .or_else(|| object.get("status"))
                .and_then(Value::as_str)
                .or_else(|| {
                    if object.get("e").and_then(Value::as_u64) == Some(0) {
                        Some("completed")
                    } else {
                        None
                    }
                }),
        )?;
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
        let context = object.get("context").cloned().unwrap_or(Value::Null);
        Ok(TaskCard {
            task_id,
            owner_session: session,
            owner_agent: agent,
            title,
            state,
            priority,
            color,
            brightness,
            progress,
            context,
            legacy_key,
            updated_at_ms: now_ms,
            reconnect_until_ms: None,
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
        }
        for task in replacement {
            self.tasks.insert(task.task_id.clone(), task);
        }
        let _ = now_ms;
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

    pub fn expire(&mut self, now_ms: u128) {
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
        self.selected.insert(device_id.to_owned(), task_id.clone());
        let task = self.tasks.get(&task_id)?;
        Some(json!({
            "type": "task_selected",
            "task_id": task_id,
            "device_id": device_id,
            "slot": slot,
            "owner_session": task.owner_session,
            "legacy_key": task.legacy_key,
            "ts": now_ms
        }))
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

    pub fn selected(&self, device_id: &str) -> Option<&str> {
        self.selected.get(device_id).map(String::as_str)
    }

    pub fn selected_slot(&self, device_id: &str) -> Option<usize> {
        let task_id = self.selected.get(device_id)?;
        let assignment = self.assignments.get(task_id)?;
        (assignment.slot.device_id == device_id).then_some(assignment.slot.slot)
    }

    pub fn selected_context(&self, device_id: &str) -> Option<&Value> {
        let task_id = self.selected.get(device_id)?;
        self.tasks.get(task_id).map(|task| &task.context)
    }

    pub fn selected_display_context(&self, device_id: &str) -> Option<Value> {
        let task_id = self.selected.get(device_id)?;
        let task = self.tasks.get(task_id)?;
        let slot = self.selected_slot(device_id)?;
        let mut context = serde_json::Map::new();
        if let Some(source) = task.context.as_object() {
            for key in [
                "project",
                "task",
                "model",
                "effort",
                "status",
                "progress",
                "task_id",
                "weekly_remaining",
                "five_hour_remaining",
            ] {
                if let Some(value) = source.get(key) {
                    context.insert(key.to_owned(), value.clone());
                }
            }
        }
        let task_title = context
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or(task.title.as_str());
        context.insert(
            "task".to_owned(),
            json!(display_task_title(slot, task_title)),
        );
        context
            .entry("status".to_owned())
            .or_insert_with(|| json!(task.state.as_str()));
        if let Some(progress) = task.progress {
            context
                .entry("progress".to_owned())
                .or_insert_with(|| json!(progress));
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
                    return json!({"id": slot, "e": 0});
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
                let color = match task.state {
                    TaskState::Queued | TaskState::Reconnecting => task.state.display_color(),
                    _ => task.color.unwrap_or_else(|| task.state.display_color()),
                };
                json!({
                    "e": u8::from(task.state != TaskState::Completed),
                    "id": slot,
                    "t": title,
                    "status": task.state.as_str(),
                    "c": color,
                    "b": f64::from(task.brightness) / 100.0,
                    "progress": task.progress,
                    "task_id": task.task_id,
                    "agent": task.owner_agent.as_str()
                })
            })
            .collect()
    }
    pub fn status_json(&self) -> Value {
        let tasks = self.tasks.values().map(|task| {
            let assignment = self.assignment(&task.task_id).map(|a| json!({"device_id": a.slot.device_id, "slot": a.slot.slot}));
            json!({"task_id": task.task_id, "owner_session": task.owner_session, "owner_agent": task.owner_agent.as_str(), "title": task.title, "state": task.state.as_str(), "priority": task.priority, "progress": task.progress, "assignment": assignment, "reconnect_until_ms": task.reconnect_until_ms})
        }).collect::<Vec<_>>();
        json!({"tasks": tasks})
    }

    fn reallocate(&mut self) {
        let available: Vec<DeviceSlot> = self.devices.values().flatten().cloned().collect();
        let available_set: HashSet<DeviceSlot> = available.iter().cloned().collect();
        self.assignments.retain(|task_id, assignment| {
            self.tasks
                .get(task_id)
                .is_some_and(|task| task.state.eligible())
                && available_set.contains(&assignment.slot)
        });
        let mut free: Vec<DeviceSlot> = available
            .into_iter()
            .filter(|slot| !self.assignments.values().any(|a| a.slot == *slot))
            .collect();
        free.sort_by(|a, b| a.device_id.cmp(&b.device_id).then(a.slot.cmp(&b.slot)));

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
        for assignment_id in self.assignments.keys() {
            if let Some(task) = self.tasks.get(assignment_id) {
                *visible_by_session.entry(task.owner_session).or_default() += 1;
            }
        }
        for slot in free {
            let Some((&session, _)) = by_session
                .iter()
                .filter(|(_, tasks)| !tasks.is_empty())
                .min_by(|(session_a, tasks_a), (session_b, tasks_b)| {
                    visible_by_session
                        .get(session_a)
                        .unwrap_or(&0)
                        .cmp(visible_by_session.get(session_b).unwrap_or(&0))
                        .then(task_order(tasks_a[0], tasks_b[0]))
                        .then(session_a.cmp(session_b))
                })
            else {
                break;
            };
            let task = by_session
                .get_mut(&session)
                .expect("session exists")
                .remove(0);
            self.assignments
                .insert(task.task_id.clone(), TaskAssignment { slot });
            *visible_by_session.entry(session).or_default() += 1;
        }
    }
}

fn task_order(a: &TaskCard, b: &TaskCard) -> Ordering {
    a.state
        .rank()
        .cmp(&b.state.rank())
        .then(b.priority.cmp(&a.priority))
        .then(b.updated_at_ms.cmp(&a.updated_at_ms))
        .then(a.task_id.cmp(&b.task_id))
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
        assert_eq!(display_task_title(0, "Build bridge"), "1 \u{2014} Build bridge");
        assert_eq!(display_task_title(5, "Build bridge"), "6 \u{2014} Build bridge");
        assert_eq!(
            display_task_title(0, "6 \u{2014} Existing label"),
            "1 \u{2014} Existing label"
        );
        assert_eq!(display_task_title(6, "Build bridge"), "Build bridge");
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
}



