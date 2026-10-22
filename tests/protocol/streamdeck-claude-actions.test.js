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
