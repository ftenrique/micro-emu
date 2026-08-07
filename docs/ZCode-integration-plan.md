---
agent: devin-cli
created: 2026-08-06T00:00:00Z
---
# ZCode Desktop (Z.ai ADE) support in the micro-emu bridge daemon

Add ZCode (Z.ai's Agentic Development Environment desktop app) as a third MCP
agent on the existing bridge daemon, and replace the current fixed 3/3 key+slot
split with a **dynamic partition** that adapts to how many agents are actually
connected.

## Current state

- The daemon (`--daemon`) owns the controller and the optional RP2040 CDC port,
  and multiplexes MCP sessions over `127.0.0.1:48360`; each agent runs
  `--mcp-proxy --agent <name>` as its stdio MCP server
  (<ref_file file="D:\Programming\micro-emu\tools\rp2040-bridge\src\daemon.rs" />,
  <ref_file file="D:\Programming\micro-emu\tools\rp2040-bridge\src\proxy.rs" />).
- `AgentId` is a two-variant enum (`Codex`, `Hermes`) with a **hard-coded**
  slot range (`slot_range`), button owner map (`button_owner`), a two-field
  queue struct (`EventRouting { codex, hermes }`) and absolute-slot LCD fusion
  (`FusedLcdState.slots[6]`)
  (<ref_snippet file="D:\Programming\micro-emu\tools\rp2040-bridge\src\routing.rs" lines="22-148" />).
- Tool exposure per agent is a `matches!` allowlist in
  <ref_snippet file="D:\Programming\micro-emu\tools\rp2040-bridge\src\mcp.rs" lines="162-171" />,
  and `bridge_status` reports a **static** `partition` object
  (<ref_snippet file="D:\Programming\micro-emu\tools\rp2040-bridge\src\main.rs" lines="625-632" />).
- ZCode supports stdio MCP servers (Settings → MCP Servers → New MCP Server,
  or JSON in `~/.zcode/config.json` / workspace `.zcode/config.json`, both
  `{"mcpServers": {...}}` and `{"server-name": {...}}` shapes), so no new
  transport is needed.

## Decisions agreed with the user

1. **Transport:** same stdio proxy as Hermes — `--mcp-proxy --agent zcode
   --autostart`. No HTTP/SSE listener.
2. **Tools for ZCode:** `bridge_status`, `poll_events`, `set_thread_status`,
   `set_rgb_config`, **plus** `set_display_context`. No Codex Micro HID tools
   (`emit_key`, `send_codex_message`, `device_status`).
3. **Dynamic partition by number of active agents**, priority
   **Codex > ZCode > Hermes**:
   - **1 active agent** → it owns all 6 keys and all 6 LCD slots.
   - **2 active agents** → split in half by priority: the higher-priority agent
     takes keys `AG00–AG02` / slots 1–3, the lower one `AG03–AG05` / slots 4–6.
   - **3 active agents** → split by **columns** (index pairs):
     Codex `AG00`+`AG03` (slots 1,4), ZCode `AG01`+`AG04` (slots 2,5),
     Hermes `AG02`+`AG05` (slots 3,6).
4. Aux keys (`ACT06–ACT08`) and encoders go to the **highest-priority active
   agent** (Codex whenever it is active, as today).

## Target architecture

```text
AJAZZ / Stream Deck ──HID── daemon rp2040-bridge ──CDC── RP2040 ──HID── ChatGPT
                                   │ (127.0.0.1:48360, JSON-RPC per line)
              ┌────────────────────┼────────────────────┐
      proxy --agent codex   proxy --agent zcode   proxy --agent hermes
              │                    │                    │
          Codex CLI            ZCode ADE        Hermes Desktop Agent
```

### Partition matrix

| Active agents            | Codex            | ZCode            | Hermes           |
|--------------------------|------------------|------------------|------------------|
| codex                    | AG00–AG05 / 1–6  | —                | —                |
| zcode                    | —                | AG00–AG05 / 1–6  | —                |
| hermes                   | —                | —                | AG00–AG05 / 1–6  |
| codex + zcode            | AG00–AG02 / 1–3  | AG03–AG05 / 4–6  | —                |
| codex + hermes           | AG00–AG02 / 1–3  | —                | AG03–AG05 / 4–6  |
| zcode + hermes           | —                | AG00–AG02 / 1–3  | AG03–AG05 / 4–6  |
| codex + zcode + hermes   | AG00,AG03 / 1,4  | AG01,AG04 / 2,5  | AG02,AG05 / 3,6  |

"Active" means: the agent has a live MCP session on the daemon **or**, for
Codex only, the RP2040 serial link is up (ChatGPT drives Codex over HID without
an MCP session). LCD slot *n* always belongs to the owner of key *AGn*.

> The 3-agent case assumes the AKP03E 3×2 grid where `AG00..AG02` is the top row
> and `AG03..AG05` the bottom row, so `{n, n+3}` is a visual column. On Stream
> Deck XL the same index pairs are used; the grouping is logical, not visual.

## Implementation steps

All changes live in `tools/rp2040-bridge/src/` except docs and scripts.

### 1. Generalize `AgentId` (`routing.rs`)

- Add `ZCode` variant; `parse` accepts `"zcode"`; `as_str` → `"zcode"`.
- Add `AGENTS: [AgentId; 3]` in priority order `[Codex, ZCode, Hermes]`,
  `fn index(self) -> usize` and `fn priority(self) -> u8`.
- Replace `EventRouting { codex, hermes }` with `queues: [EventQueue; 3]`
  indexed by `index()`; keep the existing `queue`/`queue_mut(agent)` API so
  call sites in `daemon.rs` and `main.rs` are unchanged.
- Delete `HERMES_SLOT_OFFSET` / `SLOTS_PER_AGENT` / `slot_range()` /
  `button_owner()` (superseded by step 2) and fix the call sites.

### 2. Dynamic partition (`routing.rs`)

- `ActiveSet` — a small bitset with `insert/remove/contains/len/iter` in
  priority order.
- `Partition { owners: [Option<AgentId>; 6] }` computed by
  `Partition::compute(active: ActiveSet) -> Partition`:
  - 0 active → all `None` (events dropped, LCD state retained).
  - 1 active → all six slots to that agent.
  - 2 active → `0..=2` to the higher-priority agent, `3..=5` to the other.
  - 3 active → `owners[i] = AGENTS[i % 3]` (i.e. Codex 0,3; ZCode 1,4;
    Hermes 2,5).
- Helpers: `owner_of(index) -> Option<AgentId>` (replaces `button_owner`),
  `slots_for(agent) -> Vec<usize>` (ordered, replaces `slot_range`),
  `keys_for(agent) -> Vec<String>` (`AG0n` labels for `bridge_status` /
  `initialize` instructions).
- Table-driven unit tests over all 8 subsets asserting the matrix above.

### 3. Per-agent LCD buffers (`routing.rs`)

The current `FusedLcdState` stores **absolute** slots, which breaks as soon as
an agent's slot set changes. Rework it to store **agent-local** entries:

- `FusedLcdState { entries: [[Option<Value>; LCD_SLOTS]; 3] }` — for each
  agent, up to 6 entries indexed by the position in the array that agent sent.
- `merge_from_agent(agent, parameters)` stores entry *i* at local index *i*
  (capped at 6) — no clipping at write time, so a shrink/grow of the partition
  is non-destructive.
- `fused_array(&Partition) -> Vec<Value>`: for each absolute slot, look up the
  owner and its local index (its rank within `slots_for(owner)`); unowned or
  unset slots render `{"e":0}` as today.
- `replace_full` keeps working for the controller-reconnect replay path (it
  writes into the owner's local buffer via the current partition).
- Every existing caller of `fused_array()` must now pass the partition:
  `process_codex_message`, `call_set_thread_status`, the reconnect replay.
- Side benefit: `v.oai.thstatus` from ChatGPT is no longer permanently clipped
  to 3 slots — when Codex is the only active agent it paints all six.

### 4. Repartition lifecycle (`daemon.rs`)

- Track the active set: `hello` adds, disconnect/error removes, and RP2040
  attach/detach toggles Codex.
- **Debounce**: recompute the partition ~750 ms after the last membership
  change (agents restart often; ZCode reconnects MCP servers on workspace
  switches). Store `pending_repartition_at: Option<Instant>` and resolve it in
  the main loop next to `resolve_pending_polls`.
- On repartition: recompute `Partition`, re-render `fused_array` and
  `apply_thread_status` once (the AJAZZ adapter already dedupes unchanged
  slots via `last_colors`, so there is no full-screen flicker), log the new
  ownership on stderr.
- Notify agents so they can relabel their UI: push a control event into every
  active agent's queue, surfaced by `poll_events`:
  ```json
  {"type":"partition","keys":["AG01","AG04"],"slots":[2,5],
   "agents":["codex","zcode","hermes"],"ts":1709000000000}
  ```
  Generalize `BufferedEvent` to an enum (`Key { key, pressed }` /
  `Partition { .. }`) and keep emitting key events with the existing
  `{"key","pressed","ts"}` shape (add `"type":"key"`) for compatibility.
- `initialize` instructions become dynamic per agent: list that agent's current
  keys/slots and mention that `poll_events` can deliver `type:"partition"`.

### 5. Event routing (`main.rs`, `poll_controller`)

- Replace `button_owner(index)` with `partition.owner_of(index)`.
- Codex-owned buttons keep going to HID when the RP2040 is present; otherwise
  they are buffered for the Codex MCP session (unchanged behaviour, new owner
  source).
- ZCode- and Hermes-owned buttons are buffered for their agent.
- Aux keys and encoder events go to the highest-priority active agent: HID when
  that is Codex with a live RP2040, otherwise that agent's queue.
- Unowned indices (no active agent) are dropped.

### 6. Tools and status (`mcp.rs`, `main.rs`)

- `tool_available`: allowlist per agent —
  - `codex` (and unknown/legacy `--mcp`): all tools.
  - `zcode`: `bridge_status`, `poll_events`, `set_thread_status`,
    `set_rgb_config`, `set_display_context`.
  - `hermes`: unchanged (`bridge_status`, `poll_events`, `set_thread_status`,
    `set_rgb_config`).
- `set_thread_status` description becomes partition-aware ("updates the LCD
  slots currently assigned to your agent").
- `set_display_context` and `set_rgb_config` are **device-global**: last writer
  wins. Record the writing agent and expose it in `bridge_status`
  (`displayContext.owner`, `rgbConfig.owner`) so contention is diagnosable.
- `bridge_status` gains a dynamic block:
  ```json
  {"mode":"daemon","rp2040":true,
   "agents":{"codex":{"active":true,"session":false,"events":0,
                      "keys":["AG00","AG03"],"slots":[1,4]},
             "zcode":{"active":true,"session":true,"events":2,
                      "keys":["AG01","AG04"],"slots":[2,5]},
             "hermes":{"active":false,"session":false,"events":0,
                       "keys":[],"slots":[]}},
   "partition":{"owners":["codex","zcode","hermes","codex","zcode","hermes"]}}
  ```

### 7. CLI, proxy and scripts

- `--agent` accepts `zcode`; update the error message and `--help` string in
  `parse_options` (`codex|zcode|hermes`).
- Priority is fixed for now; note `--agent-priority codex,zcode,hermes` as a
  natural later extension.
- **Autostart race:** three proxies can now race to spawn the daemon and
  `proxy.rs` has no lockfile (the Hermes plan intended one). Add a lockfile in
  `%LOCALAPPDATA%\micro-emu\bridge-daemon.lock` around `autostart_daemon`, and
  make the daemon exit quietly with a distinct message when the bind fails
  because another daemon already answers on the port.
- `package.json`: add `bridge:proxy:zcode`
  (`--mcp-proxy --agent zcode --autostart`).

### 8. Documentation

- **New** `docs/ZCode_integration.md`, mirroring `docs/Hermes_integration.md`:
  - Deployment A: ZCode only (standalone, `--daemon --port none`).
  - Deployment B: ZCode + Codex.
  - Deployment C: all three agents (the column partition).
  - Registration in ZCode — GUI (Settings → MCP Servers → New MCP Server,
    scope User, type `stdio`, command = absolute path to
    `rp2040-bridge.exe`, args `--mcp-proxy --agent zcode --autostart`) and the
    equivalent JSON for Full configuration mode / `~/.zcode/config.json`:
    ```json
    {
      "mcpServers": {
        "micro_emu_bridge": {
          "command": "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe",
          "args": ["--mcp-proxy", "--agent", "zcode", "--autostart"]
        }
      }
    }
    ```
  - Note that ZCode can also import the entry from the Codex CLI config, and
    that a workspace-scoped copy would create a second `zcode` session — the
    daemon replaces the older session, so prefer **User** scope.
  - Tool table and the `poll_events` / partition-event flow.
- Update `docs/Hermes_integration.md` and `README.md`: the fixed 3/3 table is
  replaced by the partition matrix (behaviour change for existing users).
- Update `docs/rp2040-bridge.md` with the three-agent diagram and the dynamic
  partition.

### 9. Tests

- `routing.rs`: partition matrix for all 8 active subsets; priority ordering;
  `owner_of`/`slots_for`/`keys_for`; queue array indexing.
- LCD fusion: write 6 entries as sole agent → all six rendered; add a second
  agent → first agent's entries 4–6 are hidden but restored when it becomes
  sole agent again; 3-agent column mapping (local 0→slot 0, local 1→slot 3).
- `mcp.rs`: `tools_for(Some(ZCode))` contains exactly the five allowed tools;
  `tool_available` rejects `emit_key` for `zcode`.
- `daemon.rs`: `parse_hello` for `zcode`; duplicate-session replacement;
  debounced repartition emits one partition event per active agent.
- Integration: daemon with `--port none --controller none` + three simulated
  TCP sessions doing `initialize`, `tools/list`, `set_thread_status`,
  `poll_events`, then disconnect one and assert the surviving agents receive a
  partition event with the 2-agent halves.
- Existing suites (`framing`, `messages`, `descriptor`, firmware host test)
  must stay green.

## Files to modify / create

- `tools/rp2040-bridge/src/routing.rs` — `ZCode`, agent array, `ActiveSet`,
  `Partition`, per-agent LCD buffers.
- `tools/rp2040-bridge/src/daemon.rs` — active-set tracking, debounced
  repartition, partition events, dynamic `initialize` instructions.
- `tools/rp2040-bridge/src/mcp.rs` — ZCode allowlist, partition-aware
  descriptions, event schema.
- `tools/rp2040-bridge/src/main.rs` — CLI (`zcode`), `poll_controller`
  routing, `bridge_status`, fusion call sites.
- `tools/rp2040-bridge/src/proxy.rs` — autostart lockfile.
- `package.json` — `bridge:proxy:zcode`.
- `docs/ZCode_integration.md` — **new** deployment guide.
- `docs/Hermes_integration.md`, `docs/rp2040-bridge.md`, `README.md` — updated
  partition documentation.

No new crate dependencies (`std::net`, `std::thread`, `serde_json` only).

## Verification

- [ ] `npm run bridge:test` — new and existing Rust tests.
- [ ] `npm run bridge:build` — no new warnings.
- [ ] `npm test` — JS protocol suite unchanged.
- [ ] Manual, ZCode only: `npm run bridge:daemon:standalone -- -- --controller
      ajazz`; register the stdio server in ZCode; `bridge_status` reports
      `zcode` owning all six keys; press `AG05` and receive it via
      `poll_events`; `set_thread_status` with 6 entries paints all slots.
- [ ] Manual, ZCode + Codex: RP2040 attached; both report the 3/3 halves;
      `AG00` reaches ChatGPT via HID, `AG04` reaches ZCode.
- [ ] Manual, all three: verify the column split, that each agent only sees its
      two keys, and that closing Hermes triggers a partition event and a
      re-render into halves after the debounce.
- [ ] `set_display_context` from ZCode updates the Stream Deck dashboard and
      `bridge_status` shows `displayContext.owner = "zcode"`.

## Risks / considerations

- **Behaviour change for existing users:** Codex/Hermes no longer have fixed
  halves. Mitigated by documenting the matrix and by the fact that the common
  Codex+Hermes case still yields today's 3/3 split.
- **ChatGPT is unaware of the partition:** it keeps sending 6-entry
  `v.oai.thstatus` payloads; the extra entries are stored but not rendered
  while Codex owns fewer slots. Nothing to change on the firmware side.
- **Repartition churn:** agent restarts flap the active set; the 750 ms
  debounce plus idempotent re-render keeps the LCD stable.
- **Codex activity heuristic:** treating "RP2040 up" as Codex active means a
  connected-but-idle board keeps a third of the keys. Document the
  `--port none` alternative for ZCode-first setups.
- **Global tools:** `set_rgb_config` / `set_display_context` remain
  last-writer-wins; per-agent scoping would need protocol changes.
- **ZCode workspace vs user scope:** a workspace-scoped duplicate opens a
  second `zcode` session and evicts the first; recommend User scope.
- **Autostart race** with three proxies — addressed by the lockfile in step 7.
