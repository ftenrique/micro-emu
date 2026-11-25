import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const bridgeSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/main.rs", import.meta.url),
  "utf8",
);
const contextSource = readFileSync(
  new URL("../../plugin/streamdeck/src/context.ts", import.meta.url),
  "utf8",
);
const executorSource = readFileSync(
  new URL("../../plugin/streamdeck/src/claude-action-executor.ts", import.meta.url),
  "utf8",
);

test("the mic helper offers Windows dictation into Claude Desktop", () => {
  const helper = bridgeSource.match(/fn route_desktop_dictation\(pressed: bool\) -> bool \{[\s\S]*?\n\}/);
  assert.ok(helper !== null);
  assert.match(
    helper[0],
    /claude_window::microphone_active\(\)\s*\|\|\s*crate::claude_window::is_foreground\(\)\.unwrap_or\(false\)\s*\{\s*crate::claude_window::set_microphone\(pressed\)/,
  );
});

test("long-pressing a Claude card toggles the Claude Desktop window", () => {
  assert.match(
    bridgeSource,
    /AgentId::Claude => match crate::claude_window::is_foreground\(\)[\s\S]*claude_window::minimize\(\)[\s\S]*claude_window::show_and_focus\(\)/,
  );
});

test("Claude-owned task actions route to the Claude executor and never to Codex", () => {
  assert.match(contextSource, /taskId\?\.startsWith\("claude:"\) \? "claude"/);
  assert.match(contextSource, /owner === "claude"\) \{\s*await this\.claude\.execute/);
  assert.match(executorSource, /The action was not sent to Codex/);
});

test("the Claude executor reads the last prompt and reply from the transcript", async () => {
  // Node strips the executor's type annotations, so the real module runs here.
  const { ClaudeActionExecutor, lastMessageText } = await import(
    "../../plugin/streamdeck/src/claude-action-executor.ts"
  );
  const home = mkdtempSync(join(tmpdir(), "claude-home-"));
  mkdirSync(join(home, "sessions"));
  mkdirSync(join(home, "projects", "D--repo"), { recursive: true });
  writeFileSync(join(home, "sessions", "1.json"), JSON.stringify({ pid: 1, sessionId: "s-1", hostSessionId: "local_abc" }));
  const lines = [
    { type: "user", message: { content: "first prompt" } },
    { type: "assistant", message: { content: [{ type: "text", text: "final answer" }] } },
    { type: "user", message: { content: [{ type: "tool_result", content: "ignored" }] } },
    { type: "assistant", message: { content: [{ type: "tool_use", name: "Bash" }] } },
    { type: "user", isMeta: true, message: { content: "meta" } },
  ];
  writeFileSync(join(home, "projects", "D--repo", "s-1.jsonl"), lines.map((l) => JSON.stringify(l)).join("\n"));
  const executor = new ClaudeActionExecutor(home);
  // Desktop cards carry the host id; the live registry maps it to the transcript.
  const entries = await executor.readTranscript("local_abc");
  assert.equal(lastMessageText(entries, "user"), "first prompt");
  assert.equal(lastMessageText(entries, "assistant"), "final answer");
  assert.deepEqual(await executor.readTranscript("s-1"), entries);
  await assert.rejects(executor.readTranscript("local_gone"), /no longer running/);
  await assert.rejects(
    executor.execute("task.fork", { selectedTask: { task_id: "claude:local_abc" } }),
    /not sent to Codex/,
  );
});

test("Open on a Claude card opens its desktop session, never an unvalidated id", async () => {
  const { ClaudeActionExecutor, desktopSessionId } = await import(
    "../../plugin/streamdeck/src/claude-action-executor.ts"
  );
  const opened = [];
  const executor = new ClaudeActionExecutor(tmpdir(), async (url) => { opened.push(url); });
  await executor.execute("task.open", { selectedTask: { task_id: "claude:local_7c45-6048" } });
  await executor.execute("task.open", { selectedTask: { task_id: "claude:86f78787-9abb" } });
  await executor.execute("task.open", { selectedTask: { task_id: "claude:local_a&calc" } });
  assert.deepEqual(opened, [
    "claude://code/continue?session=local_7c45-6048",
    "claude://",
    "claude://",
  ]);
  assert.equal(desktopSessionId("local_"), null);
  assert.equal(desktopSessionId(null), null);
});

test("New task starts a Claude Desktop session while Claude is focused", () => {
  const controller = readFileSync(
    new URL("../../tools/rp2040-bridge/src/plugin_controller.rs", import.meta.url),
    "utf8",
  );
  assert.match(
    controller,
    /claude_window::is_foreground\(\)\.unwrap_or\(false\) \{\s*match crate::claude_window::start_new_session\(\)/,
  );
  const window = readFileSync(
    new URL("../../tools/rp2040-bridge/src/claude_window.rs", import.meta.url),
    "utf8",
  );
  assert.match(window, /open_deep_link\("claude:\/\/code\/new"\)/);
});

test("usage screens follow the last pressed agent, Claude included", () => {
  const actionSource = readFileSync(
    new URL("../../plugin/streamdeck/src/actions/context.ts", import.meta.url),
    "utf8",
  );
  const inspector = readFileSync(
    new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/ui/context.html", import.meta.url),
    "utf8",
  );
  const crux = readFileSync(
    new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/ui/crux-v.html", import.meta.url),
    "utf8",
  );
  for (const html of [inspector, crux]) {
    assert.match(html, /setting="usageAgent" default="auto"/);
    assert.match(html, /value="claude"/);
  }
  // Unknown or missing settings follow the selection; known agents pin.
  assert.match(actionSource, /value === "codex" \|\| value === "zcode" \|\| value === "claude" \? value : "auto"/);
  assert.match(contextSource, /setting === "auto" \? this\.getUsageAgent\(\) : setting/);
  // The plugin no longer pins the bridge's source on connect or appear.
  assert.doesNotMatch(contextSource, /sendUsageAgent/);
  assert.doesNotMatch(actionSource, /setUsageAgent/);
  // The bridge switches the source on every task press and navigation.
  assert.match(bridgeSource, /follow_usage_to_task\(bridge, &task\);\s*\/\/ Auto-fed ZCode cards/);
  assert.match(bridgeSource, /pending_task_events\.push\(\(session, selection\)\);\s*\}\s*follow_usage_to_task/);
});

test("the bridge renews an expired Claude login without clobbering Claude Code", () => {
  const usage = readFileSync(new URL("../../tools/rp2040-bridge/src/usage.rs", import.meta.url), "utf8");
  assert.match(usage, /https:\/\/platform\.claude\.com\/v1\/oauth\/token/);
  assert.match(usage, /"grant_type": "refresh_token"/);
  // Re-read before writing, and replace the file through a rename.
  assert.match(usage, /stored_refresh != Some\(refresh_token\.as_str\(\)\)/);
  assert.match(usage, /std::fs::rename\(&temp, path\)/);
  // Rejected refresh tokens are remembered by fingerprint only.
  assert.match(usage, /REJECTED_REFRESH_TOKEN[\s\S]*token_fingerprint/);
});
