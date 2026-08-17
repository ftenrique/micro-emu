# Hermes Desktop Agent integration

This guide covers deploying the micro-emu bridge with the
[Hermes Desktop Agent](https://hermes-agent.nousresearch.com/docs/user-guide/desktop)
from Nous Research. The bridge can serve Hermes on its own (standalone, no
RP2040 required) or simultaneously with Codex/ChatGPT through a shared
daemon.

## Architecture

```text
                         ┌─ Codex CLI / ChatGPT (HID + MCP)
                         │
AJAZZ / Stream Deck ──HID── bridge daemon ──CDC── RP2040 ──HID──┘
                                │ (127.0.0.1:48360, JSON-RPC)
                    ┌───────────┴───────────┐
            proxy (codex)           proxy (hermes)
                    │                       │
                Codex CLI           Hermes Desktop Agent
```

The daemon owns the physical controller and (optionally) the RP2040 serial
port. Each agent launches a lightweight STDIO proxy that connects to the
daemon over TCP loopback, identifies itself with a hello line, and pumps
JSON-RPC lines bidirectionally. The daemon multiplexes all sessions against
a single hardware-owning loop.

### Key and LCD partition

The six-key primary controller uses **fixed halves** so connecting another
agent never moves an existing Codex card: Codex owns `AG00`-`AG02`; ZCode has
priority on `AG03`-`AG05`; Hermes owns `AG03`-`AG05` only while ZCode is
absent. Auxiliary task devices remain available to the shared scheduler.

| Active agents            | Codex            | ZCode            | Hermes           |
|--------------------------|------------------|------------------|------------------|
| codex only               | AG00–AG02 / 0–2  | —                | —                |
| hermes only              | —                | —                | AG03–AG05 / 3–5  |
| codex + hermes           | AG00–AG02 / 0–2  | —                | AG03–AG05 / 3–5  |
| zcode + hermes           | —                | AG03–AG05 / 3–5  | —                |
| codex + zcode + hermes   | AG00–AG02 / 0–2  | AG03–AG05 / 3–5  | —                |

"Active" means the agent has a live MCP session on the daemon, or — for
Codex only — the RP2040 serial link is up **and** has forwarded a Codex
status frame (`v.oai.thstatus`) within the last 60 seconds, or — for Hermes —
the Hermes desktop app is running (the bridge mirrors the app's session
database directly, so Hermes keeps its cards and its half of the deck across
MCP proxy reconnects and daemon restarts; closing the app releases them).
When the active set changes, the daemon debounces for 750 ms and then
recomputes the partition. Each active agent receives a **partition event**
via `poll_events` notifying it of its new keys and slots. LCD state is
retained through repartitions.

See the [ZCode integration guide](./ZCode_integration.md) for the full
partition matrix with all three agents.

LCD status slots are **fused**: each agent only writes its assigned range,
and the physical controller always displays the combined six-slot state.

## Prerequisites

- Windows with PowerShell 5.1+.
- Rust 1.85+ with Cargo (or a prebuilt `rp2040-bridge.exe`).
- Hermes Agent installed (`hermes` on PATH or in `~/.hermes`). See the
  [Hermes install guide](https://hermes-agent.nousresearch.com/docs/user-guide/install).
- For the physical controller: an AJAZZ AKP03E (`0300:3002`) or a supported
  Stream Deck (Plus `0FD9:0084`, Plus XL, or original XL `0FD9:006C`).
- For the Codex + RP2040 path: an RP2040 Zero board with the micro-emu
  firmware flashed (see the main
  [README](../README.md#flash-the-rp2040)).

## Build the bridge

From the repository root:

```powershell
npm run bridge:build
```

The release binary is at:

```text
tools\rp2040-bridge\target\release\rp2040-bridge.exe
```

Run the tests to confirm everything compiles:

```powershell
npm run bridge:test
```

---

## Deployment A: Hermes only (standalone, no RP2040)

Use this mode when you want to drive Hermes with the physical controller but
do not need ChatGPT/Codex Micro HID emulation. No RP2040 board is required.

### 1. Start the daemon

```powershell
npm run bridge:daemon:standalone -- -- --controller ajazz
```

Or with the compiled binary directly:

```powershell
.\tools\rp2040-bridge\target\release\rp2040-bridge.exe `
  --daemon --port none --controller ajazz
```

Replace `ajazz` with `streamdeck-plus`, `streamdeck-plus-xl`, or
`streamdeck-xl` if you are using a Stream Deck. Use `--controller none` for
testing without a physical device.

The daemon prints a `bridge-ready` line and listens on `127.0.0.1:48360`:

```json
{"type":"bridge-ready","firmware":"standalone","port":"none","rp2040":false,
 "controller":{"kind":"ajazz","connected":true},"mode":"daemon",
 "agents":{"codex":{"events":0,"keys":[],"slots":[]},
           "zcode":{"events":0,"keys":[],"slots":[]},
           "hermes":{"events":0,"keys":["AG03","AG04","AG05"],"slots":[3,4,5]}},
 "partition":{"owners":[null,null,null,"hermes","hermes","hermes"]}}
```

### 2. Register the proxy with Hermes

Edit `~/.hermes/config.yaml` (on Windows: `%LOCALAPPDATA%\hermes\config.yaml`; create it if it does not exist):

```yaml
mcp_servers:
  micro_emu_bridge:
    command: "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe"
    args: ["--mcp-proxy", "--agent", "hermes", "--autostart"]
    enabled: true
    timeout: 120
    connect_timeout: 60
```

With `--autostart`, the proxy spawns the daemon automatically if it is not
already running. If you started the daemon manually in step 1, you can omit
`--autostart`.

> **Tip:** If `rp2040-bridge.exe` is not on Hermes's `PATH`, use the absolute
> path as shown above. Backslashes must be doubled in YAML strings.

### 3. Reload MCP in Hermes

```powershell
hermes
```

Inside the Hermes TUI, run `/reload-mcp` after editing the config. Verify the
connection with `/tools` — you should see `bridge_status`, `poll_events`,
`set_thread_status`, `publish_tasks`, and `set_display_context`.

### 4. Verify

Ask Hermes to call `bridge_status`. You should see the daemon report
`mode: "daemon"`, `rp2040: false`, and your controller as connected.

Press `AG03` on the physical controller, then ask Hermes to call
`poll_events` (or `poll_events` with `timeout_ms: 5000` for a long-poll). You
should receive:

```json
{"events": [{"type": "key", "key": "AG03", "pressed": true, "ts": 1709000000000}]}
```

---

## Deployment B: Hermes + Codex simultaneously

Use this mode when you want both ChatGPT (via the Codex Micro HID interface
on the RP2040) and Hermes Desktop Agent to share the same physical
controller.

### 1. Flash the RP2040 (one-time)

Follow the main
[README](../README.md#flash-the-rp2040) to build and flash the firmware.
Find the CDC port:

```powershell
npm run rp2040:port
```

### 2. Start the daemon

```powershell
npm run bridge:daemon -- -- --port auto --controller ajazz
```

Or with the binary:

```powershell
.\tools\rp2040-bridge\target\release\rp2040-bridge.exe `
  --daemon --port auto --controller ajazz
```

The daemon opens the AJAZZ/Stream Deck HID interface and the RP2040 CDC
port. ChatGPT sees the Codex Micro HID device as before. The daemon also
listens on `127.0.0.1:48360` for MCP proxy connections.

Close the official AJAZZ/Stream Deck software before starting the daemon so
it can open the HID interface exclusively.

### 3. Register the proxy with Hermes

Same as Deployment A, step 2:

```yaml
mcp_servers:
  micro_emu_bridge:
    command: "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe"
    args: ["--mcp-proxy", "--agent", "hermes", "--autostart"]
    enabled: true
    timeout: 120
```

### 4. Register the proxy with Codex

Edit `%USERPROFILE%\.codex\config.toml` (or a project-scoped
`.codex/config.toml`):

```toml
[mcp_servers.micro_emu_bridge]
command = "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe"
args = ["--mcp-proxy", "--agent", "codex", "--autostart"]
cwd = "D:\\Programming\\micro-emu"
startup_timeout_sec = 20
tool_timeout_sec = 60
enabled = true
```

Or register via the Codex CLI:

```powershell
codex mcp add micro_emu_bridge `
  -- D:\Programming\micro-emu\tools\rp2040-bridge\target\release\rp2040-bridge.exe `
     --mcp-proxy --agent codex --autostart
```

Verify with `codex mcp list` and `/mcp` in the Codex TUI.

### 5. Verify both agents

1. Start ChatGPT — it should detect Codex Micro via the RP2040 HID.
2. Start Hermes — run `/reload-mcp` and `/tools`.
3. Ask each agent to call `bridge_status`. Both should report
   `mode: "daemon"` and `rp2040: true`.
4. Press `AG00` — ChatGPT/Codex receives it via HID.
5. Press `AG03` — call `poll_events` from Hermes to receive it.
6. Call `set_thread_status` from Hermes with 3 entries — LCD slots 4-6
   update. Call it from Codex with 3 entries — slots 1-3 update. The
   controller shows all six slots fused.

---

## Using the bridge with Hermes

### Available tools (Hermes agent)

| Tool               | Description                                                        |
|--------------------|--------------------------------------------------------------------|
| `bridge_status`    | Report daemon, firmware, serial port, controller, and agent state. |
| `poll_events`      | Drain buffered physical key presses (`AG03`-`AG05`). Long-poll with `timeout_ms`. |
| `set_thread_status`| Update LCD slots 4-6 with color/brightness entries.                |
| `publish_tasks`    | Publish a stable-ID task snapshot with lifecycle and metadata.     |
| `set_display_context` | Publish model, effort, progress, and usage dashboard metadata.  |

### Automatic session state

While a Hermes proxy is connected **or the Hermes desktop app is running**,
the daemon opens Hermes' canonical `state.db` read-only and mirrors the most
recent non-archived sessions as task cards. The desktop app is probed every
two seconds, so the feed survives MCP proxy blips and daemon restarts for as
long as the app stays open. On Windows the default is
`%LOCALAPPDATA%\hermes\state.db`; elsewhere it is `$HERMES_HOME/state.db` or
`~/.hermes/state.db`. The adapter publishes stable `hermes:<session-id>` IDs,
title, workspace, model, lifecycle, and exact turn timestamps. Missing,
locked, or older databases are ignored without clearing manually published
cards.

An explicit `publish_tasks` call from Hermes immediately takes precedence over
the auto-feed and remains authoritative through the normal reconnect lease.
Use `set_display_context` when Hermes has live usage or effort metadata that is
not present in the session database.

### Selecting sessions from the deck (Windows)

Pressing an auto-fed Hermes card selects it on the board **and switches the
Hermes desktop app to that session** through Windows UI Automation, so the
press works even with no Hermes proxy connected. The automation clicks the
sidebar row for the session; if the sidebar is collapsed it is opened for the
selection and collapsed again afterwards, and if the row sits behind the
sidebar's pagination the search box surfaces it (the filter is cleared
afterwards). Selections are queued on a worker thread — UIA can take seconds
on a cold accessibility tree — with a burst of presses collapsing into the
most recent one and failed attempts retried up to three times. Activation is
confirmed by watching the session's editor tab become active, so a slow
switch is never reported as success before it happened.

Long-pressing a Hermes card shows or focuses the Hermes window and minimizes
it when it is already foreground. If Windows refuses the foreground
activation, the bridge falls back to relaunching `Hermes.exe`: Electron's
single-instance handshake asks the running app to raise its own window.

### New task and Mic keys (Windows)

The Stream Deck **New task** action (`agent.new-task`) is Hermes-aware. On
press, the plugin asks the daemon whether a desktop agent is the foreground
window; when it is the Hermes window, the daemon sends Hermes'
**Ctrl+N** accelerator — the shortcut carried by the sidebar's
*New session* button — so a new session starts immediately, without the
UIA warm-up the ZCode path needs. With anything else focused (or no daemon
connection) the action opens the Codex new-task screen as before. The daemon
replies over the controller socket (`new-task-result`), so the key keeps its
OK/alert feedback, and an older bridge simply times out into the Codex
fallback.

The **Mic** action (Codex Micro ACT10, `encoder-button` index 2) is
Hermes-aware too. Hermes has no voice input of its own, so while its window
is the foreground app the mic key drives Windows' built-in dictation
instead: **press** first selects Hermes' message composer through Windows UI
Automation, then sends `Win+H`; dictation transcribes into that composer
without requiring a manual click; **release** sends `Escape`, closing the dictation bar
(hold-to-talk semantics). With anything else focused the mic key keeps its
original ChatGPT/Codex behavior. The bridge remembers that it opened the
dictation bar, so the release always closes it even if the dictation UI
itself grabbed the foreground or you switched windows mid-hold.

### `poll_events`

Hermes cannot receive push notifications from MCP servers (MCP is
pull-based). The bridge buffers physical key presses for the Hermes partition
and exposes them through `poll_events`. Partition change events are also
delivered through this mechanism, notifying Hermes when its key/slot
assignment changes.

**Immediate drain (returns instantly):**

```json
{"method": "tools/call", "params": {"name": "poll_events", "arguments": {}}}
```

**Long-poll (waits up to 25 seconds for events):**

```json
{"method": "tools/call", "params": {"name": "poll_events", "arguments": {"timeout_ms": 25000}}}
```

Response:

```json
{"events": [
  {"type": "key", "key": "AG03", "pressed": true, "ts": 1709000000000},
  {"type": "key", "key": "AG03", "pressed": false, "ts": 1709000000120}
]}
```

Each press generates two events: `pressed: true` (key down) and
`pressed: false` (key up). The `ts` field is a Unix epoch millisecond
timestamp. Partition change events have `"type": "partition"` and include
`keys`, `slots`, and `agents` fields.

The event queue is capped at 256 events per agent; the oldest events are
dropped first if the queue overflows.

### `set_thread_status`

Updates the LCD slots currently assigned to Hermes. Use `bridge_status` to
see which slots you own. Each entry is an object with at minimum an `e` field
(enabled: `0` = off, `1` = on) and optionally `t` (title), `c` (color), and
`b` (brightness).

```json
{"method": "tools/call", "params": {"name": "set_thread_status", "arguments": {
  "status": [
    {"i": 0, "e": 1, "t": "Hermes", "c": "#FF6600", "b": 100},
    {"i": 1, "e": 1, "t": "working", "c": "#00AAFF", "b": 80},
    {"i": 2, "e": 0}
  ]
}}}
```

The `i` field is relative to the agent's slot range (0-based within your
assigned slots). Slots with `e: 0` or `b: 0` are cleared. The bridge fuses
this with other agents' entries and applies the combined six-slot state to
the physical controller.

### `bridge_status`

```json
{
  "type": "bridge-ready",
  "firmware": "rp2040-zero/0.1.1-diag",
  "port": "COM7",
  "rp2040": true,
  "controller": {"kind": "ajazz", "connected": true, "model": "AKP03E", "serial": null},
  "agents": {
    "codex": {"events": 0, "keys": ["AG00","AG01","AG02"], "slots": [0,1,2]},
    "zcode": {"events": 0, "keys": [], "slots": []},
    "hermes": {"events": 3, "keys": ["AG03","AG04","AG05"], "slots": [3,4,5]}
  },
  "partition": {
    "owners": ["codex","codex","codex","hermes","hermes","hermes"]
  },
  "mode": "daemon"
}
```

---

## CLI reference

### Daemon mode

```
rp2040-bridge.exe --daemon --port auto|COMx|none [options]
```

| Flag                | Default             | Description                                      |
|---------------------|---------------------|--------------------------------------------------|
| `--daemon`          |                     | Run as TCP daemon (multi-agent).                 |
| `--port`            | `auto`              | `auto`, `COMx`, or `none` (standalone).          |
| `--bind`            | `127.0.0.1:48360`   | TCP bind address (loopback only recommended).    |
| `--controller`      | `ajazz`             | `ajazz`, `streamdeck-plus`, `streamdeck-plus-xl`, `streamdeck-xl`, or `none`. |
| `--controller-serial` |                   | Select a specific device by serial number.       |
| `--no-ajazz`        |                     | Alias for `--controller none`.                   |

### Proxy mode

```
rp2040-bridge.exe --mcp-proxy --agent codex|zcode|hermes [options]
```

| Flag                | Default             | Description                                      |
|---------------------|---------------------|--------------------------------------------------|
| `--mcp-proxy`       |                     | Run as STDIO-to-TCP proxy.                       |
| `--agent`           | (required)          | `codex`, `zcode`, or `hermes`.                   |
| `--connect`         | `127.0.0.1:48360`   | Daemon TCP address to connect to.                |
| `--autostart`       |                     | Spawn the daemon if it is not running.           |
| `--daemon-args`     |                     | Space-separated args for the autostarted daemon. |

### Legacy STDIO mode (single agent, backward compatible)

```
rp2040-bridge.exe --mcp --port auto
```

This is the original single-owner STDIO mode. It does not support multiple
agents. Use the daemon + proxy mode instead when you need both Codex and
Hermes.

---

## npm scripts

| Script                       | Description                                          |
|------------------------------|------------------------------------------------------|
| `bridge:daemon`              | Start the daemon with `--port auto`.                 |
| `bridge:daemon:standalone`   | Start the daemon with `--port none` (no RP2040).     |
| `bridge:proxy:codex`         | Start the Codex proxy with `--autostart`.            |
| `bridge:proxy:zcode`         | Start the ZCode proxy with `--autostart`.            |
| `bridge:proxy:hermes`        | Start the Hermes proxy with `--autostart`.           |
| `bridge:build`               | Build the release binary.                            |
| `bridge:test`                | Run the Rust test suite.                             |

---

## Troubleshooting

### The daemon does not start

- Check that port `48360` is not already in use:
  `Get-NetTCPConnection -LocalPort 48360 -ErrorAction SilentlyContinue`.
- If the AJAZZ/Stream Deck software is running, close it before starting the
  daemon so it can open the HID interface.
- Run with `--controller none` first to isolate hardware issues.

### Hermes cannot connect to the bridge

- Verify the `command` path in the Hermes `config.yaml` points to the correct
  `.exe` location. Use backslashes doubled in YAML.
- Run `/reload-mcp` in Hermes after editing the config.
- Check Hermes logs in `~/.hermes/logs/` for MCP connection errors.
- Test the proxy manually:
  ```powershell
  .\tools\rp2040-bridge\target\release\rp2040-bridge.exe --mcp-proxy --agent hermes
  ```
  Then type a JSON-RPC line on stdin and check the daemon stderr for the
  connection.

### `poll_events` returns no events

- Use `bridge_status` to check which keys are assigned to Hermes. Only
  your assigned keys generate events for your queue.
- In standalone mode (`--port none`), Codex keys are also buffered for the
  Codex MCP session. If no Codex proxy is connected, those events sit in the
  Codex queue and are not visible to Hermes.
- The event queue caps at 256 events. If Hermes polls infrequently, old
  events may be dropped.

### LCD slots do not update

- `set_thread_status` from Hermes only writes the slots currently assigned to
  Hermes. Use `bridge_status` to check your slot assignment. When only Codex
  and Hermes are active, Hermes owns slots 4-6.
- An entry with `e: 0` or `b: 0` clears the slot. Make sure your entries
  have `e: 1` and a non-zero brightness.
- The controller must be connected. Check `bridge_status` →
  `controller.connected`.

### Multiple proxies try to autostart the daemon

- The `--autostart` flag uses a lockfile in
  `%LOCALAPPDATA%\micro-emu\bridge-daemon.lock` to prevent race conditions
  when multiple proxies start simultaneously.
- If you still see issues, start the daemon manually before launching any
  agents:
  ```powershell
  npm run bridge:daemon -- -- --port auto
  ```
  Then omit `--autostart` from both proxy configs.

### RP2040 disconnects after system resume

- The daemon retries the serial connection with exponential backoff
  (100 ms → 1 s) when the RP2040 disappears. It keeps the MCP sessions alive
  during the reconnection. No manual intervention is needed.
- If the COM port number changes after resume, use `--port auto` so the
  daemon re-resolves by VID/PID.

---

## Security notes

- The daemon binds to `127.0.0.1` (loopback) only. It does not expose the
  TCP port to the network.
- No authentication is applied to the loopback connection. Any local process
  can connect and call MCP tools. This is acceptable for a single-user
  desktop; for shared machines, restrict access at the OS level.
- The bridge does not log prompt bodies or task content from the CDC
  transport.

## Task publishing and combined devices

Hermes sessions share one contention-free task board with Codex and ZCode. Publish a snapshot with stable task ids using `publish_tasks`; each result contains the current `{device_id, slot}` assignment or `null` for overflow. Task states are `queued`, `running`, `waiting`, `error`, `paused`, and `completed`, with priority/progress from 0 to 100.

Configure combined capacity with repeatable daemon options, for example `--device ajazz,serial=AJ-1 --device streamdeck-plus,serial=SD-1`. AJAZZ contributes six slots, Stream Deck+ eight, and XL devices default to eight (override with `task-slots=N`). All eight Stream Deck+ keys select task cards in daemon mode; legacy `set_thread_status` remains available as a session-local adapter.

Selections arrive through `poll_events` as `task_selected`, and reallocation is reported with `layout_changed`. `bridge_status` version 2 includes sessions, device health, assignments/overflow, per-device selection, queue depth, and the 30-second reconnect lease. RGB is centrally configured by the daemon, not individual agents.

Long-pressing a Hermes task card shows or focuses Hermes Desktop, and minimizes it when it is already foreground. Pressing a card selects it and switches the Hermes desktop app to that session through UI automation (see *Selecting sessions from the deck* above). Search, terminal, and model-cycle actions for Hermes tasks are queued as agent events; they are never sent to Codex. Workspace-path copying works locally. Copy-prompt, copy-response, and fork use Hermes' Sessions REST API only when MICRO_EMU_HERMES_API_URL and MICRO_EMU_HERMES_API_KEY are configured. Unsupported Hermes actions fail closed instead of falling through to the Codex executor.
