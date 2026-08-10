# ZCode Desktop integration

This guide covers deploying the micro-emu bridge with
[ZCode](https://zcode.z.ai/en/docs/welcome), the Agentic Development
Environment from Z.ai. The bridge can serve ZCode on its own (standalone, no
RP2040 required), simultaneously with Codex/ChatGPT, or together with both
Codex and Hermes through a shared daemon.

## Architecture

```text
                         ┌─ Codex CLI / ChatGPT (HID + MCP)
                         │
AJAZZ / Stream Deck ──HID── bridge daemon ──CDC── RP2040 ──HID──┘
                                │ (127.0.0.1:48360, JSON-RPC)
              ┌─────────────────┼─────────────────┐
      proxy (codex)     proxy (zcode)     proxy (hermes)
              │                │                  │
          Codex CLI        ZCode ADE      Hermes Desktop Agent
```

The daemon owns the physical controller and (optionally) the RP2040 serial
port. Each agent launches a lightweight STDIO proxy that connects to the
daemon over TCP loopback, identifies itself with a hello line, and pumps
JSON-RPC lines bidirectionally. The daemon multiplexes all sessions against
a single hardware-owning loop.

### Dynamic key and LCD partition

The six LCD keys are partitioned **dynamically** based on which agents are
currently active. Priority order is **Codex > ZCode > Hermes**.

| Active agents            | Codex            | ZCode            | Hermes           |
|--------------------------|------------------|------------------|------------------|
| codex only               | AG00–AG05 / 1–6  | —                | —                |
| zcode only               | —                | AG00–AG05 / 1–6  | —                |
| hermes only              | —                | —                | AG00–AG05 / 1–6  |
| codex + zcode            | AG00–AG02 / 1–3  | AG03–AG05 / 4–6  | —                |
| codex + hermes           | AG00–AG02 / 1–3  | —                | AG03–AG05 / 4–6  |
| zcode + hermes           | —                | AG00–AG02 / 1–3  | AG03–AG05 / 4–6  |
| codex + zcode + hermes   | AG00,AG03 / 1,4  | AG01,AG04 / 2,5  | AG02,AG05 / 3,6  |

"Active" means the agent has a live MCP session on the daemon, or — for Codex
only — the RP2040 serial link is up (ChatGPT drives Codex over HID without an
MCP session).

When the active set changes (agent connects, disconnects, or the RP2040 is
attached/detached), the daemon waits 750 ms (debounce) and then recomputes the
partition. Each active agent receives a **partition event** via `poll_events`
notifying it of its new keys and slots. LCD state is retained through
repartitions: if an agent's slot set grows, its previously painted entries
reappear.

The 3-agent column split assumes the AKP03E 3×2 grid where `AG00..AG02` is the
top row and `AG03..AG05` the bottom row, so `{n, n+3}` is a visual column.

## Prerequisites

- Windows with PowerShell 5.1+.
- Rust 1.85+ with Cargo (or a prebuilt `rp2040-bridge.exe`).
- ZCode installed. Download from
  [zcode.z.ai/en](https://zcode.z.ai/en) and complete the onboarding.
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

## Deployment A: ZCode only (standalone, no RP2040)

Use this mode when you want to drive ZCode with the physical controller but
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

The daemon prints a `bridge-ready` line and listens on `127.0.0.1:48360`.
Since ZCode is the only active agent, it owns all six keys and slots:

```json
{"type":"bridge-ready","firmware":"standalone","port":"none","rp2040":false,
 "controller":{"kind":"ajazz","connected":true},"mode":"daemon",
 "agents":{"codex":{"events":0,"keys":[],"slots":[]},
           "zcode":{"events":0,"keys":["AG00","AG01","AG02","AG03","AG04","AG05"],"slots":[0,1,2,3,4,5]},
           "hermes":{"events":0,"keys":[],"slots":[]}},
 "partition":{"owners":["zcode","zcode","zcode","zcode","zcode","zcode"]}}
```

### 2. Register the proxy with ZCode

Open ZCode, go to **Settings → MCP Servers**, and click **New MCP Server**.

- **Scope:** User (available in all workspaces)
- **Name:** `micro_emu_bridge`
- **Type:** `stdio`
- **Command:** `D:\Programming\micro-emu\tools\rp2040-bridge\target\release\rp2040-bridge.exe`
- **Arguments:** `--mcp-proxy`, `--agent`, `zcode`, `--autostart`

Alternatively, switch to **Full configuration** mode and paste:

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

Or edit `~/.zcode/config.json` directly with the same JSON structure.

With `--autostart`, the proxy spawns the daemon automatically if it is not
already running. If you started the daemon manually in step 1, you can omit
`--autostart`.

> **Tip:** ZCode can also import MCP servers from Codex CLI's
> `~/.codex/config.toml` via the **Import** button on the MCP Servers page.
> If you already have the bridge registered for Codex, import it and change
> the `--agent` argument to `zcode`.

### 3. Verify

Confirm the server is enabled in the MCP Servers list. Ask ZCode Agent to
call `bridge_status`. You should see the daemon report `mode: "daemon"`,
`rp2040: false`, and ZCode owning all six keys.

Press `AG00` on the physical controller, then ask ZCode to call `poll_events`
(or `poll_events` with `timeout_ms: 5000` for a long-poll). You should
receive:

```json
{"events": [{"type": "key", "key": "AG00", "pressed": true, "ts": 1709000000000}]}
```

Call `set_thread_status` with 6 entries — all six LCD slots update since
ZCode is the sole agent.

---

## Deployment B: ZCode + Codex simultaneously

Use this mode when you want both ChatGPT (via the Codex Micro HID interface
on the RP2040) and ZCode to share the same physical controller.

### 1. Flash the RP2040 (one-time)

Follow the main [README](../README.md#flash-the-rp2040) to build and flash
the firmware. Find the CDC port:

```powershell
npm run rp2040:port
```

### 2. Start the daemon

```powershell
npm run bridge:daemon -- -- --port auto --controller ajazz
```

The daemon opens the AJAZZ/Stream Deck HID interface and the RP2040 CDC port.
ChatGPT sees the Codex Micro HID device as before. The daemon also listens on
`127.0.0.1:48360` for MCP proxy connections.

Close the official AJAZZ/Stream Deck software before starting the daemon so
it can open the HID interface exclusively.

### 3. Register the proxy with ZCode

Same as Deployment A, step 2.

### 4. Register the proxy with Codex

Edit `%USERPROFILE%\.codex\config.toml`:

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

### 5. Verify both agents

1. Start ChatGPT — it should detect Codex Micro via the RP2040 HID.
2. Start ZCode — it should connect to the bridge MCP server.
3. Ask each agent to call `bridge_status`. Both should report `mode: "daemon"`
   and `rp2040: true`. The partition shows Codex with `AG00–AG02` / slots 1–3
   and ZCode with `AG03–AG05` / slots 4–6.
4. Press `AG00` — ChatGPT/Codex receives it via HID.
5. Press `AG03` — call `poll_events` from ZCode to receive it.
6. Call `set_thread_status` from ZCode with 3 entries — LCD slots 4–6 update.
   Call it from Codex with 3 entries — slots 1–3 update. The controller shows
   all six slots fused.

---

## Deployment C: all three agents (Codex + ZCode + Hermes)

When all three agents are active, the partition switches to the **column
split**: Codex gets `AG00`+`AG03` (slots 1, 4), ZCode gets `AG01`+`AG04`
(slots 2, 5), Hermes gets `AG02`+`AG05` (slots 3, 6).

### 1. Start the daemon

Same as Deployment B, step 2.

### 2. Register proxies

Register all three proxies:

- **Codex:** `--mcp-proxy --agent codex --autostart` (in `.codex/config.toml`)
- **ZCode:** `--mcp-proxy --agent zcode --autostart` (in ZCode Settings → MCP)
- **Hermes:** `--mcp-proxy --agent hermes --autostart` (in `~/.hermes/config.yaml`)

See the [Hermes integration guide](./Hermes_integration.md) for Hermes-specific
setup.

### 3. Verify the column split

1. Ensure all three agents are connected (check `bridge_status` → `agents`).
2. The `partition.owners` array should be
   `["codex","zcode","hermes","codex","zcode","hermes"]`.
3. Press `AG01` — ZCode receives it via `poll_events`.
4. Press `AG03` — Codex receives it via HID.
5. Press `AG05` — Hermes receives it via `poll_events`.
6. Each agent's `set_thread_status` writes only its 2 assigned slots.

### 4. Disconnect one agent

When an agent disconnects (e.g., you close Hermes), the daemon debounces for
750 ms and then repartitions into a 2-agent half split. The surviving agents
receive a partition event via `poll_events`:

```json
{"type": "partition", "keys": ["AG00","AG01","AG02"], "slots": [0,1,2],
 "agents": ["codex","zcode"], "ts": 1709000000750}
```

---

## Using the bridge with ZCode

### Available tools (ZCode agent)

| Tool                 | Description                                                        |
|----------------------|--------------------------------------------------------------------|
| `bridge_status`      | Report daemon, firmware, serial port, controller, and agent state. |
| `poll_events`        | Drain buffered physical key presses for your keys. Long-poll with `timeout_ms`. Also delivers partition change events. |
| `set_thread_status`  | Update the LCD slots currently assigned to your agent.             |
| `set_rgb_config`     | Send `v.oai.rgbcfg` configuration to the controller.               |
| `set_display_context`| Update the Stream Deck + dashboard with project, task, model and effort metadata. |

### `poll_events`

ZCode (like all MCP clients) cannot receive push notifications from MCP
servers. The bridge buffers physical key presses for your partition and
exposes them through `poll_events`.

**Immediate drain (returns instantly):**

```json
{"method": "tools/call", "params": {"name": "poll_events", "arguments": {}}}
```

**Long-poll (waits up to 25 seconds for events):**

```json
{"method": "tools/call", "params": {"name": "poll_events", "arguments": {"timeout_ms": 25000}}}
```

Key event response:

```json
{"events": [
  {"type": "key", "key": "AG01", "pressed": true, "ts": 1709000000000},
  {"type": "key", "key": "AG01", "pressed": false, "ts": 1709000000120}
]}
```

Partition change event response:

```json
{"events": [
  {"type": "partition", "keys": ["AG01","AG04"], "slots": [1,4],
   "agents": ["codex","zcode","hermes"], "ts": 1709000000750}
]}
```

Each key press generates two events: `pressed: true` (key down) and
`pressed: false` (key up). The `ts` field is a Unix epoch millisecond
timestamp.

The event queue is capped at 256 events per agent; the oldest events are
dropped first if the queue overflows.

### `set_thread_status`

Updates the LCD slots currently assigned to your agent. Use `bridge_status`
to see which slots you own. Each entry is an object with at minimum an `e`
field (enabled: `0` = off, `1` = on) and optionally `t` (title), `c` (color),
and `b` (brightness).

```json
{"method": "tools/call", "params": {"name": "set_thread_status", "arguments": {
  "status": [
    {"i": 0, "e": 1, "t": "ZCode", "c": "#00AAFF", "b": 100},
    {"i": 1, "e": 1, "t": "building", "c": "#FF6600", "b": 80}
  ]
}}}
```

The `i` field is relative to your slot range (0-based within your assigned
slots). The bridge fuses your entries with other agents' entries and applies
the combined state to the physical controller.

### `set_display_context`

Updates the Stream Deck + dashboard with project, task, model, and effort
metadata. This tool is only meaningful when using a Stream Deck Plus or Plus
XL as the physical controller.

```json
{"method": "tools/call", "params": {"name": "set_display_context", "arguments": {
  "project": "my-app",
  "task": "refactoring auth module",
  "model": "GLM-5.2",
  "effort": "high",
  "status": "running",
  "progress": 42,
  "weekly_remaining": 73,
  "five_hour_remaining": 28
}}}
```

This is a device-global setting: the last agent to call it wins. Use
`bridge_status` to check the current `displayContext`.

### `bridge_status`

```json
{
  "type": "bridge-ready",
  "firmware": "rp2040-zero/0.1.1-diag",
  "port": "COM7",
  "rp2040": true,
  "controller": {"kind": "ajazz", "connected": true, "model": "AKP03E", "serial": null},
  "agents": {
    "codex": {"events": 0, "keys": ["AG00","AG03"], "slots": [0,3]},
    "zcode": {"events": 2, "keys": ["AG01","AG04"], "slots": [1,4]},
    "hermes": {"events": 0, "keys": ["AG02","AG05"], "slots": [2,5]}
  },
  "partition": {
    "owners": ["codex","zcode","hermes","codex","zcode","hermes"]
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

| Flag                  | Default             | Description                                      |
|-----------------------|---------------------|--------------------------------------------------|
| `--daemon`            |                     | Run as TCP daemon (multi-agent).                 |
| `--port`              | `auto`              | `auto`, `COMx`, or `none` (standalone).          |
| `--bind`              | `127.0.0.1:48360`   | TCP bind address (loopback only recommended).    |
| `--controller`        | `ajazz`             | `ajazz`, `streamdeck-plus`, `streamdeck-plus-xl`, `streamdeck-xl`, or `none`. |
| `--controller-serial` |                     | Select a specific device by serial number.       |
| `--no-ajazz`          |                     | Alias for `--controller none`.                   |

### Proxy mode

```
rp2040-bridge.exe --mcp-proxy --agent codex|zcode|hermes [options]
```

| Flag              | Default             | Description                                      |
|-------------------|---------------------|--------------------------------------------------|
| `--mcp-proxy`     |                     | Run as STDIO-to-TCP proxy.                       |
| `--agent`         | (required)          | `codex`, `zcode`, or `hermes`.                   |
| `--connect`       | `127.0.0.1:48360`   | Daemon TCP address to connect to.                |
| `--autostart`     |                     | Spawn the daemon if it is not running.           |
| `--daemon-args`   |                     | Space-separated args for the autostarted daemon. |

### Legacy STDIO mode (single agent, backward compatible)

```
rp2040-bridge.exe --mcp --port auto
```

This is the original single-owner STDIO mode. It does not support multiple
agents. Use the daemon + proxy mode instead when you need ZCode alongside
other agents.

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

### ZCode cannot connect to the bridge

- Verify the `command` path in ZCode Settings → MCP Servers points to the
  correct `.exe` location. Backslashes must be doubled in JSON strings.
- After editing the configuration, restart ZCode or toggle the MCP server
  off and on again.
- Test the proxy manually:
  ```powershell
  .\tools\rp2040-bridge\target\release\rp2040-bridge.exe --mcp-proxy --agent zcode
  ```
  Then type a JSON-RPC line on stdin and check the daemon stderr for the
  connection.

### `poll_events` returns no events

- Use `bridge_status` to check which keys are assigned to your agent. Only
  your assigned keys generate events for your queue.
- In standalone mode (`--port none`), if no other agent is connected, you
  own all six keys and all events should reach your queue.
- The event queue caps at 256 events. If you poll infrequently, old events
  may be dropped.

### LCD slots do not update

- `set_thread_status` only writes the slots currently assigned to your agent.
  Use `bridge_status` to check your slot assignment.
- An entry with `e: 0` or `b: 0` clears the slot. Make sure your entries
  have `e: 1` and a non-zero brightness.
- The controller must be connected. Check `bridge_status` →
  `controller.connected`.

### Multiple proxies try to autostart the daemon

- The `--autostart` flag uses a lockfile in
  `%LOCALAPPDATA%\micro-emu\bridge-daemon.lock` to prevent race conditions.
  If the lockfile is stale (older than 30 seconds), it is removed
  automatically.
- If you still see issues, start the daemon manually and omit `--autostart`
  from the proxy configurations.

### Partition changes unexpectedly

- The partition is dynamic: connecting or disconnecting an agent changes
  which keys/slots each agent owns. The daemon debounces for 750 ms before
  applying the change.
- Codex is considered active whenever the RP2040 is connected, even without
  an MCP session. If you want ZCode to own all keys, use `--port none` so
  Codex is not active.
- Check `bridge_status` → `partition.owners` to see the current assignment.

## Task publishing and combined devices

The daemon treats task instances—not product names—as ownership units, so multiple ZCode sessions and other agent kinds can coexist. Publish a complete snapshot from each session:

```json
{"tasks":[{"task_id":"auth-refactor","title":"Refactor authentication","state":"running","priority":50,"color":"#4488FF","progress":42,"context":{"project":"micro-emu","model":"gpt-5","effort":"high"}}]}
```

The response reports the stable assignment for every task (`device_id` plus physical `slot`) or `null` when the combined board is full. Use repeatable daemon options such as `--device ajazz,serial=AJ-1 --device streamdeck-plus,serial=SD-1`; capacities are six AJAZZ slots and eight Stream Deck+ slots. XL models default to eight and accept `task-slots=N`.

Task selection is delivered to the owning session as `task_selected`; `layout_changed` is emitted after reflow. `bridge_status` version 2 reports sessions, devices, assignments, overflow, selected task per device, queue depth, and reconnect leases. A disconnect retains cards for 30 seconds, allowing the same stable task ids to reclaim their slots. The touch-strip context follows the selected card, while RGB remains centrally daemon-managed.