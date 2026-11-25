# Claude integration

This guide covers driving [Claude Code](https://docs.claude.com/en/docs/claude-code)
— the CLI and the sessions hosted by the Claude desktop app's **Code** tab —
from the micro-emu bridge. Claude joins the same daemon as Codex, ZCode and
Hermes and follows the same model: an MCP proxy for tools, plus a read-only
auto-feed that mirrors live sessions onto the task board.

## Key and LCD partition

Codex keeps the fixed first half (`AG00`–`AG02` / slots 0–2). The second half
(`AG03`–`AG05` / slots 3–5) goes to the highest-priority active agent among
**Claude, ZCode and Hermes, in that order**.

| Active agents                    | Codex            | Claude           | ZCode            | Hermes           |
|----------------------------------|------------------|------------------|------------------|------------------|
| claude only                      | —                | AG03–AG05 / 3–5  | —                | —                |
| codex + claude                   | AG00–AG02 / 0–2  | AG03–AG05 / 3–5  | —                | —                |
| claude + zcode and/or hermes     | —                | AG03–AG05 / 3–5  | —                | —                |
| codex + claude + zcode + hermes  | AG00–AG02 / 0–2  | AG03–AG05 / 3–5  | —                | —                |

"Active" for Claude means a Claude MCP proxy is connected **or** the Claude
desktop app has a window open (probed every two seconds). Closing the app and
disconnecting the proxy releases the half to ZCode/Hermes after the usual
750 ms repartition debounce. On an eight-slot Stream Deck the automatic task
layout shares slots among all active agents in priority order
(Codex, Claude, ZCode, Hermes).

## Automatic session state

Every running Claude Code process writes a small record to
`~/.claude/sessions/<pid>.json` (or `$CLAUDE_CONFIG_DIR/sessions`) with its
session name, working directory and `busy`/`idle` status. While Claude is
active the daemon reads those records once per second and publishes the most
recent sessions as task cards:

| Record                                   | Card state  |
|------------------------------------------|-------------|
| `status: "busy"`                         | `running` (timer starts at `statusUpdatedAt`) |
| waiting / permission statuses            | `waiting`   |
| `status: "idle"` after a turn            | `completed` (green until selected) |
| `status: "idle"` on a fresh session      | `queued`    |

Cards use stable `claude:<id>` identities: the desktop host session id
(`local_…`) when present, so `/clear` keeps the card in place, otherwise the
Claude Code session id. Records whose process has exited (or whose pid was
reused, checked against the recorded process start time) are ignored, as are
non-interactive (`claude -p`) runs. The card's model comes from the latest
assistant reply in the session transcript
(`~/.claude/projects/<project>/<session-id>.jsonl`, last 64 KB only).

The bridge never writes to Claude's files. An explicit `publish_tasks` call
from a Claude proxy takes precedence over the auto-feed, exactly as for ZCode
and Hermes.

## Register the MCP proxy

```powershell
claude mcp add --scope user micro_emu_bridge -- `
  D:\Programming\micro-emu\tools\rp2040-bridge\target\release\rp2040-bridge.exe `
  --mcp-proxy --agent claude --autostart
```

Or add it to `~/.claude.json` / a project `.mcp.json`:

```json
{
  "mcpServers": {
    "micro_emu_bridge": {
      "command": "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe",
      "args": ["--mcp-proxy", "--agent", "claude", "--autostart"]
    }
  }
}
```

The proxy is optional: with the desktop app open, cards appear without it.
Claude gets the same agent-neutral tools as ZCode and Hermes:
`bridge_status`, `poll_events`, `set_thread_status`, `publish_tasks` and
`set_display_context`. `npm run bridge:proxy:claude` starts the proxy from a
source checkout.

## Deck behaviour (Windows)

- **Short press** on a Claude card selects it on the board (and delivers a
  `task_selected` event to a connected Claude proxy) and switches Claude
  Desktop to that session through the app's own deep link,
  `claude://code/continue?session=local_<id>`, which also raises the window.
  Only sessions hosted by the desktop app (`local_…` ids) can be opened this
  way; a card for a session started from a terminal selects on the board only.
- **Long press** raises the Claude desktop window, or minimizes it when it is
  already in the foreground. If Windows refuses the foreground switch, the
  bridge opens `claude://`, which hands the request to the running app.
- **Mic** (ACT10): while Claude Desktop is the foreground window, press
  focuses its message composer and opens Windows dictation (`Win+H`); release
  closes the dictation bar — the same hold-to-talk behaviour as ZCode and
  Hermes.
- **Open** (and the Context key's Task press) on a selected Claude card
  opens that session in Claude Desktop through the same deep link; a
  terminal-only session just brings the app forward.
- **New task** while Claude Desktop is the foreground window opens a new
  Code-tab session through `claude://code/new`.
- **Copy path / Copy prompt / Copy response** on a selected Claude card read
  the local transcript. Retry, Fork, Archive, Review, Run tests, Compact and
  Settings have no Claude integration; they fail closed and are never sent
  to Codex.

## Usage limits

The usage screens (Context key in Usage mode, the Crux V strip, the Stream
Deck + / AJAZZ strips) show the limits of the **last agent whose task card
was pressed** or navigated to. Pressing a Claude card switches them to
Claude's 5-hour and weekly windows; pressing a Codex or ZCode card switches
back. Hermes has no usage source, so pressing a Hermes card leaves the
previous agent's limits on screen (the strip label always names the agent).

In the Stream Deck property inspector, **Usage Source** defaults to *Last
pressed agent*. Picking Codex, ZCode or Claude pins that key or dial to one
agent instead. Keys saved before this option existed may still be pinned to
Codex; switch them to *Last pressed agent* to follow the selection.

Claude usage comes from the same endpoint Claude Code's `/usage` screen
reads, `GET https://api.anthropic.com/api/oauth/usage`, authorized with the
access token Claude Code stores in `~/.claude/.credentials.json`
(`$CLAUDE_CONFIG_DIR` if set).

Sessions hosted by the desktop app authenticate through the app and never
renew that file, so the bridge renews it itself. Once the stored access
token has expired it makes Claude Code's own refresh-token grant
(`POST https://platform.claude.com/v1/oauth/token`, Claude Code's client id
and the login's stored scopes) and writes the new tokens back:

- Only the `claudeAiOauth` token fields change; every other field is kept.
- The file is replaced in one step (temp file, then rename), so readers
  never see a partial file. Claude Code watches the file and picks up the
  new tokens.
- If Claude Code renewed the file while the request was in flight, its
  token is used and the bridge's result is dropped.
- A refresh token the server rejects is not retried until the file
  changes; the reason is logged (`Claude token refresh rejected ...`).

`rp2040-bridge --print-usage claude` runs one fetch the way the daemon
does and prints only the parsed percentages.

## Not supported yet

- A login whose refresh token the server no longer accepts (signed out,
  revoked, or replaced by a newer login) cannot be renewed: run `claude`
  in a terminal and sign in with `/login` once, after which the bridge keeps
  it renewed.
