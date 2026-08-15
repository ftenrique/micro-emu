# Multi-Agent Coexistence: Why One Agent Monopolizes the Deck, and How to Fix It

> **Status (2026-08-14): resolved.** All four root causes below have been
> implemented and the symptom is gone. This document is kept as the design
> rationale; the cited line numbers were accurate as of 2026-08-10 and have
> drifted since. One planned detail changed during implementation: the
> partition uses **fixed halves** (Codex AG00–AG02, ZCode AG03–AG05, Hermes
> AG03–AG05 only while ZCode is absent) rather than the 1-agent-owns-all /
> 3-agent column split described here — see `Partition::compute` in
> `routing.rs` and the tables in `ZCode_integration.md` /
> `Hermes_integration.md` for the current model.

**Audience:** an implementation agent (or developer) who has not seen this codebase before.
Every claim below cites exact files and line numbers (as of 2026-08-10). Read the cited code
before changing anything.

---

## 1. The symptom

The bridge is designed to let up to 3 agents (Codex, ZCode, Hermes) share one 6-key
Stream Deck-style controller. In practice, one agent's task cards end up occupying **all six
keys**, and the other agents never get their assigned keys/slots rendered. Input routing also
behaves as if the partition is wrong (e.g. a 3-way split when only 2 agents are running).

## 2. Architecture recap (what is *supposed* to happen)

Two cooperating systems exist in `tools/rp2040-bridge/src/`:

1. **Partition system** (`routing.rs`):
   - `Partition::compute(ActiveSet)` assigns each of the 6 keys/LCD
     slots an owner based on which agents are active. (This document was
     written against the older dynamic model — 1 agent owning all 6,
     a 3-agent column split `i % 3`. The shipped implementation uses fixed
     halves; see the status note above and `routing.rs`.)
   - `EventRouting::route_button` (`routing.rs:309-325`) delivers key presses only to the
     slot's owner.
   - `FusedLcdState` (`routing.rs:354-421`) merges each agent's `thstatus` writes so an agent
     can only paint its own slots.
   - The daemon recomputes the partition (debounced 750 ms, `daemon.rs:32`) whenever the
     active set changes and notifies every agent with a `partition` event
     (`daemon.rs:582-614`).

2. **Task board system** (`tasks.rs`), enabled in daemon mode via `bridge.task_mode = true`
   (`daemon.rs:152`). Tasks published by any session go into one shared `TaskBoard`, and
   `TaskBoard::reallocate()` (`tasks.rs:611-665`) maps tasks onto physical slots.

The problem: **these two systems disagree about who owns the slots, and the task board wins.**

## 3. Root causes (ranked by impact)

### RC1 — The task board completely ignores the partition (primary cause)

`desired_primary_cards` (`main.rs:512-537`) chooses what to render:

```rust
if bridge.task_mode {
    if bridge.has_explicit_task_state || bridge.last_thread_status.is_some() {
        if bridge.task_board.has_tasks() || bridge.last_thread_status.is_none() {
            bridge.task_board.rendered_slots(&bridge.task_device_id, bridge.task_slot_count)
        } else {
            bridge.fused_lcd.fused_array(&bridge.partition)   // partition-aware fallback
        }
    } ...
```

As soon as the task board has *any* tasks, rendering goes through
`TaskBoard::rendered_slots` (`tasks.rs:567-602`), which reads its own
`assignments` map and **never consults `bridge.partition`**.

`TaskBoard::reallocate()` (`tasks.rs:611-665`) fills every free slot from the global
candidate list. Its "fairness" (`tasks.rs:643-653`) only balances among **sessions that
currently have eligible tasks**. Consequences:

- If Codex publishes 6 tasks and ZCode/Hermes are connected but momentarily have no
  eligible tasks, Codex gets **all 6 slots**. The partition says ZCode owns slots 1,4 and
  Hermes owns 2,5 — but nothing enforces it.
- Once assigned, tasks keep their slots (`tasks.rs:614-619` only evicts ineligible tasks),
  so a late-arriving agent finds every slot already taken until the hog's tasks complete.
- Fairness keys on `owner_session`, not `owner_agent` (`tasks.rs:632-634`). One agent
  with multiple sessions (allowed — see `daemon.rs:302`) gets multiple fairness shares.

Meanwhile the **input side still uses the partition** (`route_button`), so key presses go to
agents whose cards are not even displayed — the visible/interactive mapping diverges.

### RC2 — Codex is force-activated by hardware presence

`effective_active_set` (`daemon.rs:125-131`) inserts Codex into the active set whenever the
RP2040 serial link is up, even with no Codex MCP session:

```rust
fn effective_active_set(session_agents: ActiveSet, codex_hardware_active: bool) -> ActiveSet {
    let mut set = session_agents;
    if codex_hardware_active { set.insert(AgentId::Codex); }
    set
}
```

This is intentional (ChatGPT drives Codex over HID without MCP, comment at
`daemon.rs:122-124`), but it means: with the RP2040 plugged in, ZCode + Hermes never get a
clean 2-way split — Codex phantom-occupies slots 0 and 3 forever, and serial
attach/detach (`daemon.rs:575-580`) churns the partition for everyone.

### RC3 — Codex-flavored defaults leak everywhere

- `connection_default_cards` (`main.rs:476-496`) hardcodes `"agent": "codex"` on every
  standby card (line 492), and `desired_primary_cards` sizes them using
  `partition.slots_for(AgentId::Codex)` (`main.rs:525-530`) — Codex-only logic in a
  supposedly shared path.
- `connection_default_context` (`main.rs:498-510`) hardcodes `"model": "CODEX"`.
- `auto_derive_display_context` (reads Codex CLI config/session files, `main.rs:562+`) is
  invoked on **every** agent's Hello (`daemon.rs:224-227`), on Codex thstatus
  (`daemon.rs:378-379`), during the ZCode poll (`daemon.rs:567-569`), and every 5 minutes
  (`daemon.rs:627-629`). A ZCode or Hermes connection therefore repaints the display
  context from Codex's config.

### RC4 — Coordination is advisory, not enforced

Agents receive `partition` events telling them which keys/slots they own
(`routing.rs:328-346`), and `set_thread_status` uses relative slot indexing so the fused
path stays inside bounds. But `publish_tasks` has **no cap** tied to the partition: any
agent may publish any number of tasks, and the daemon happily schedules them onto slots the
partition assigned to someone else. The system relies on every agent voluntarily behaving,
and the scheduler doesn't referee.

## 4. Fix plan

Do the steps in order. Each step is independently verifiable. All paths are relative to
`D:\Programming\micro-emu\tools\rp2040-bridge\src\`.

### Step 1 — Make `TaskBoard::reallocate()` partition-aware (fixes RC1, the big one)

1. Give `TaskBoard` access to slot ownership. Add a field and setter in `tasks.rs`:

   ```rust
   /// owner of each physical slot index, mirroring Partition::owners.
   slot_owners: Vec<Option<AgentId>>,   // empty = unrestricted (legacy/tests)

   pub fn set_slot_owners(&mut self, owners: Vec<Option<AgentId>>) {
       if self.slot_owners != owners {
           self.slot_owners = owners;
           self.assignments.clear();   // force clean re-layout under new ownership
           self.reallocate();
       }
   }
   ```

2. In the daemon, call `bridge.task_board.set_slot_owners(...)` right after
   `bridge.partition = new_partition.clone()` in the repartition block
   (`daemon.rs:593`), building the vector from `partition.owner_of(i)` for `i in 0..6`.

3. In `reallocate()` (`tasks.rs:611-665`), enforce ownership in **both** phases:
   - Retention (`tasks.rs:614-619`): also drop an assignment if
     `slot_owners[assignment.slot.slot] != Some(task.owner_agent)` (when `slot_owners`
     is non-empty and the slot index is in range).
   - Assignment loop (`tasks.rs:642-656`): for each free slot, restrict the candidate
     sessions to those whose next task's `owner_agent` matches
     `slot_owners[slot.slot]`. If no candidate matches, leave the slot empty
     (`{"id": slot, "e": 0}` renders as OFF — correct behavior for an idle agent's slot).

4. Group fairness by `owner_agent` first, then `owner_session`, so one agent with two
   sessions doesn't get a double share (`tasks.rs:632-634`).

Note: `slot_owners` maps only the primary device's slots. Aux/plugin controller slots
(other `device_id`s in `self.devices`) should remain unrestricted — check
`slot.device_id == primary` before applying ownership, or key `slot_owners` by device.

### Step 2 — Stop force-activating Codex on serial presence (fixes RC2)

The cheapest correct behavior: only treat hardware as "Codex active" if Codex traffic has
actually been seen recently. The daemon already tracks `last_thstatus_at`
(`daemon.rs:183`). Change the daemon so that instead of
`codex_hardware_active = bridge.has_serial()`, it is:

```rust
codex_hardware_active = bridge.has_serial()
    && last_thstatus_at.is_some_and(|t| now.duration_since(t) < CODEX_HARDWARE_IDLE);
```

with `CODEX_HARDWARE_IDLE` around 60 s. Recompute this in the main loop where
`now_has_serial` is currently compared (`daemon.rs:575-580`) and keep the existing
repartition debounce. This way an idle plugged-in RP2040 no longer steals a third of the
deck from ZCode/Hermes, but ChatGPT-driven Codex still claims its slots as soon as it
sends a `thstatus` frame. Optionally add a `--codex-hardware-always` flag to restore the
old behavior.

### Step 3 — De-Codex the shared defaults (fixes RC3)

1. `connection_default_cards` (`main.rs:476-496`): take the partition and tag each card
   with its owner (`partition.owner_of(i)`), falling back to `"e": 0` for unowned slots.
   Remove the hardcoded `"agent": "codex"`.
2. `desired_primary_cards` (`main.rs:524-531`): size defaults from the full partition, not
   `slots_for(AgentId::Codex)`.
3. Gate `auto_derive_display_context` calls in `daemon.rs` on the agent being Codex:
   at Hello (`daemon.rs:224-227`) only when `info.agent == AgentId::Codex`; leave the
   thstatus path (`daemon.rs:378`) as-is (it is Codex by definition); drop it from the
   ZCode poll path (`daemon.rs:567-569`) or make it a no-op when Codex is not in the
   active set.

### Step 4 — Enforce per-agent task quotas at publish time (fixes RC4)

In the daemon's `publish_tasks` handler (`daemon.rs:~1124`) and the ZCode auto-feed
(`daemon.rs:~551`), clamp the number of *eligible* tasks per agent to
`bridge.partition.slots_for(agent).len()` before inserting into the task board (keep the
highest-ranked by `task_order`, `tasks.rs:668-675`). With Step 1 in place this is
belt-and-braces, but it keeps queue state honest and `task_board_status` output truthful.

### Step 5 — Tests

Add tests in `tasks.rs` (existing test style at bottom of file) and daemon integration
tests (`daemon.rs:1366+` shows the pattern):

1. **Partition respected:** 3 agents active, Codex publishes 6 tasks, ZCode publishes 1,
   Hermes publishes 0 → Codex tasks appear only on slots {0,3}, ZCode's on {1,4},
   slots {2,5} render `{"e":0}`.
2. **Repartition reflow:** start with Codex-only (owns all 6, 6 tasks visible), Hermes
   connects → after repartition Codex shows only its 3 slots, Hermes' 3 are empty/OFF.
3. **No phantom Codex:** serial up, no thstatus for > idle window, ZCode + Hermes
   sessions → partition is a 2-way split with no Codex slots.
4. **Multi-session agent:** two Codex sessions + one Hermes session → per-agent fairness,
   Codex collectively limited to its partition slots.

Run: `cargo test` in `tools\rp2040-bridge` (PowerShell:
`cargo test --manifest-path tools\rp2040-bridge\Cargo.toml`).

### Acceptance criteria

- With all 3 agents active, each agent's cards appear **only** on its partition slots;
  pressing a key always reaches the agent whose card is shown on it.
- An agent with zero tasks keeps its slots reserved (rendered OFF), not stolen.
- Unplugging/replugging the RP2040 while ZCode+Hermes run does not repartition unless
  Codex actually sends traffic.
- ZCode/Hermes connecting never triggers a Codex-config-derived display repaint.

## 5. Non-goals / cautions

- Do **not** change the wire protocol, the `partition` event schema, or the relative slot
  indexing of `set_thread_status` — agent-side integrations depend on them
  (see `docs/ZCode_integration.md`, `docs/Hermes_integration.md`).
- Do not touch `FusedLcdState` — it is already partition-correct; the bug is that the task
  board path bypasses it.
- Keep the 750 ms `REPARTITION_DEBOUNCE` (`daemon.rs:32`); removing it re-introduces
  churn during reconnect storms.
- Aux/plugin controller slots (`plugin_controller.rs`) are outside the 6-slot partition;
  leave their allocation unrestricted.
