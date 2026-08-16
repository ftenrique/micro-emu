# Stream Deck action catalog

Task Cards, physical Codex Micro controls, and extended actions use separate routes. AG00-AG05 remain Task Card positions; an Action Button never selects an AG code.

## Execution routes

| Route | What happens |
|---|---|
| Micro | The plugin sends the existing HID or encoder command to the bridge. |
| Bridge | The daemon changes task selection or sends a documented Codex desktop shortcut. |
| Codex | The plugin invokes a Codex deep link or calls the locally installed Codex app-server. |

Codex actions no longer queue an unhandled catalog_action event. A successful request flashes the Stream Deck success indicator; a rejected or unavailable request flashes an alert and is written to the plugin log.

On Windows, app-server actions use the npm Codex CLI. Install @openai/codex, or set MICRO_EMU_CODEX_CLI to an absolute codex.js or codex.exe path. MICRO_EMU_NODE_EXE can override the Node executable used for codex.js.

## Available actions

### Direct Codex Micro controls

- micro.act06, micro.act07, micro.act08
- micro.mic and micro.send
- micro.rotor-click, micro.rotor-cw, micro.rotor-ccw
- micro.up, micro.down, micro.left, micro.right

These remain configurable through Codex Micro. AG00-AG05 are deliberately absent because Task Cards own those visible positions. When no RP2040 serial link is available, `micro.mic` keeps the same press/release behavior by focusing Codex and holding `Ctrl+Shift+M` until the Stream Deck key is released.

### Task navigation

- task.previous
- task.next
- task.first
- task.last

Navigation wraps across occupied Task Cards and never emits a Micro key.

### Selected Codex task

- task.open: open the selected task through the Codex deep link
- task.retry: replay the latest user input as a new turn
- task.fork: fork the selected task and open the result
- task.archive: archive the selected task
- task.copy-prompt: copy the latest user prompt
- task.copy-response: copy the latest final agent response
- task.copy-path: copy the task working directory

### Codex workflow

- agent.new-task: start a new task while the ZCode or Hermes desktop app is
  the foreground window; otherwise open the Codex new-task screen
- agent.search: focus Codex and open task search
- agent.review-changes: start an inline review of uncommitted changes
- agent.run-tests: start a turn that runs and diagnoses relevant tests
- agent.open-terminal: focus Codex and toggle its terminal
- agent.compact-context: start task context compaction
- agent.settings: open Codex settings

## Retired actions

The picker does not offer operations that the installed Codex protocol cannot execute across clients: interrupt, pin, unpin, approve, reject, open browser, and open editor.

Old Stream Deck profiles keep a tombstone for these identifiers. Pressing one shows an alert; it never falls back to ACT06 or silently queues a no-op event.

## Appearance and migration

Every action has an automatic icon and short title. The property inspector exposes icon and title overrides; clearing either returns to the catalog default. Pre-catalog numeric settings remain safe: indices 6-8 map to ACT06-ACT08, while indices 0-5 retain hidden legacy AG behavior until the user chooses a catalog action.

### Crux dials

The Crux Horizontal and Vertical dials expose the same catalog as the Action Button for their dial press. `Native press` (the default) keeps the original Codex Micro behavior — the dial's own encoder button, ACT12 / Send on the horizontal dial and ACT10 / Mic on the vertical one — and every catalog group above is selectable the same way, including task navigation and Codex workflow actions. The touch strip label shows the assigned action's short title, and press feedback matches the Action Button: a success flash for Codex workflow actions and an alert when the bridge is offline or the action is rejected.

Pre-catalog click tokens (`ag0`-`ag5`, `act6`-`act8`, `mic`, `send`, `rotor`) keep working: they resolve to their catalog equivalents (or the legacy AG Micro keys) at runtime, even though the property inspector no longer lists them.

### Context button

The `Context` key is available on regular Stream Deck Keypad controllers. Choose `Task info`, `Model info`, or `Usage info` in the property inspector. It mirrors the corresponding Stream Deck+ LCD context screen; task mode opens Codex task search, model mode cycles the configured featured model, and pressing usage mode toggles between the remaining percentages and the 5-hour/weekly reset times.

In `Usage info` mode a second setting, `Usage Source`, picks which agent reports the limits: `Codex` (default) or `ZCode`. The bridge fetches Codex limits from the ChatGPT backend API (`~/.codex/auth.json`) and ZCode limits from the Z.AI monitor endpoint (key from `~/.zcode/v2/config.json`) — no MCP calls are involved. Every display context carries both agents' snapshots (`agents_usage`), so each usage display renders its own configured source and two displays can show Codex and ZCode at the same time. The Context key's choice is also pushed to the daemon as the global source for the hardware LCD strip (labeled `LIMITS CODEX`/`LIMITS ZCODE`); the daemon defaults to Codex and the selection is re-sent on reconnect. The Crux Vertical dial has the same `Usage Source` setting for its touch strip, scoped to that dial only.
