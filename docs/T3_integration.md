# T3 Code integration

This guide covers driving **T3 Code Desktop** from the `micro-emu` bridge and Stream Deck plugin. T3 Code joins Codex, Claude, ZCode, and Hermes on the shared dynamic task board with a strictly read-only SQLite auto-feed, provider harness labels, provider-aware usage switching, and custom T3 Code actions.

## Dynamic equal-priority task slots

Auto task buttons no longer reserve fixed slices per agent or give any agent higher priority than another:

- As Codex, Claude, ZCode, Hermes, and T3 Code sessions become active, they dynamically claim unused `Auto` task buttons on demand.
- Active work (`waiting`, `error`, `running`, `thinking`) always takes priority over inactive work. When all buttons are occupied, settled threads are evicted and reused first, followed by older finished/idle buttons.
- Explicit per-slot overrides configured in the Stream Deck **Task Card** Property Inspector (`Codex`, `Claude`, `ZCode`, `Hermes`, or `T3 Code`) remain pinned to their chosen agent while unpinned slots stay in the shared dynamic pool.

## Automatic thread state (read-only)

While T3 Code Desktop is running (or a `--mcp-proxy --agent t3` proxy is connected), the daemon polls `~/.t3/userdata/statev2.sqlite` (or `$T3CODE_HOME/userdata/statev2.sqlite`) strictly read-only (`SQLITE_OPEN_READ_ONLY`, never writing to or locking T3 Code's database):

| T3 Code thread state | Card state | Priority tier |
|---|---|---|
| Pending runtime approval / input request | `waiting` | Active (highest) |
| Active run (`running` / `starting` / `in_progress`) | `running` | Active |
| Unsettled failed run (`failed` / `error`) | `error` | Active |
| Completed unsettled thread | `completed` (green until selected) | Idle / finished |
| Settled thread (`settledAt` or `settledOverride = "settled"`) | `queued` | Settled (shown when free slots exist; evicted first) |

Imported threads (`import:*`), archived/deleted threads, and child subagent threads (`lineage.parentThreadId`) are excluded automatically so only top-level T3 Code threads appear on the deck.

## Provider harness labels & usage tracking

Each T3 Code task card displays the underlying provider harness instead of a generic label:

- `codex` → **`T3-Codex`**
- `claudeAgent` → **`T3-Claude`**
- `acpRegistry_google_antigravity` → **`T3-AGY`**
- `acpRegistry_devin` → **`T3-Devin`**
- `acpRegistry_glm_*` → **`T3-GLM`**
- Other ACP / custom providers → **`T3-<Provider>`**

The usage strip follows the **last provider pressed** on any task button:

- Pressing a `T3-Codex` card switches the usage display to **Codex** limits.
- Pressing a `T3-Claude` card switches the usage display to **Claude** limits.
- Pressing a `T3-GLM` card switches the usage display to **ZCode / Z.ai** limits.
- Pressing a provider without a usage fetcher (such as `T3-AGY` or `T3-Devin`) leaves the current usage source in place rather than blanking the display.

## Deck behaviour (Windows)

- **Short press** on a T3 Code card selects it on the board, updates the usage source for its provider, and opens that thread in T3 Code Desktop via `t3code://threads/<environmentId>/<threadId>` (with strict alphanumeric/dash ID validation).
- **Long press** raises T3 Code Desktop, or minimizes it when it is already in the foreground.
- **Mic** (`ACT10`): while T3 Code Desktop is in the foreground, press focuses the composer and opens Windows dictation (`Win+H`), and release closes dictation (`Esc`).
- **New task** (`agent.new-task`): when T3 Code Desktop is in the foreground, opens `t3code://new`.
- **Open / Copy workspace path / Copy prompt / Copy last response**: supported on any selected T3 Code task card (prompt and response copying reads the latest message via `rp2040-bridge --read-t3-message <threadId> user|assistant`).
- **Search tasks / Open terminal / Model cycle**: on a selected T3 Code card, trigger T3 Code's project search (`Ctrl+Shift+F`), integrated terminal (`Ctrl+J`), and model picker (`Ctrl+Shift+M`).

## Custom T3 Code actions

The Stream Deck **Action Button** and **Crux Horizontal / Vertical** dial push pickers include a dedicated **T3 Code** action group:

| Action ID | Label | Behaviour |
|---|---|---|
| `t3.new-thread` | T3: New thread | Opens `t3code://new` |
| `t3.command-palette` | T3: Command palette | Focuses T3 Code and sends `Ctrl+K` |
| `t3.model-picker` | T3: Pick model | Focuses T3 Code and sends `Ctrl+Shift+M` |
| `t3.settle-thread` | T3: Settle thread | Focuses T3 Code and sends `Ctrl+Shift+S` |
| `t3.pin-thread` | T3: Pin thread | Focuses T3 Code and sends `Ctrl+Shift+P` |
| `t3.toggle-preview` | T3: Toggle preview | Focuses T3 Code and sends `Ctrl+Shift+J` |
| `t3.toggle-sidebar` | T3: Toggle sidebar | Focuses T3 Code and sends `Ctrl+B` |
| `t3.toggle-terminal` | T3: Toggle terminal | Focuses T3 Code and sends `Ctrl+J` |
| `t3.copy-reference` | T3: Copy thread ref | Focuses T3 Code and sends `Ctrl+Shift+C` |
| `t3.open-usage` | T3: Open usage | Focuses T3 Code and sends `Ctrl+U` |

## Optional MCP proxy

T3 Code threads appear automatically whenever T3 Code Desktop is running. You can also attach an MCP proxy explicitly:

```powershell
npm run bridge:proxy:t3
```

Or run the compiled bridge binary directly with `--mcp-proxy --agent t3 --autostart`.
