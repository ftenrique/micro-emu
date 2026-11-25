import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const bridgeSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/main.rs", import.meta.url),
  "utf8",
);
const daemonSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/daemon.rs", import.meta.url),
  "utf8",
);
const usageSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/usage.rs", import.meta.url),
  "utf8",
);
const codexSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/codex.rs", import.meta.url),
  "utf8",
);
const pluginControllerSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/plugin_controller.rs", import.meta.url),
  "utf8",
);
const contextSource = readFileSync(
  new URL("../../plugin/streamdeck/src/context.ts", import.meta.url),
  "utf8",
);
const catalogSource = readFileSync(
  new URL("../../plugin/streamdeck/src/action-catalog.ts", import.meta.url),
  "utf8",
);
const taskHtml = readFileSync(
  new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/ui/task.html", import.meta.url),
  "utf8",
);
const actionHtml = readFileSync(
  new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/ui/action.html", import.meta.url),
  "utf8",
);
const cruxHHtml = readFileSync(
  new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/ui/crux-h.html", import.meta.url),
  "utf8",
);
const cruxVHtml = readFileSync(
  new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/ui/crux-v.html", import.meta.url),
  "utf8",
);

const T3_ACTION_IDS = [
  "t3.new-thread",
  "t3.command-palette",
  "t3.model-picker",
  "t3.settle-thread",
  "t3.pin-thread",
  "t3.toggle-preview",
  "t3.toggle-sidebar",
  "t3.toggle-terminal",
  "t3.copy-reference",
  "t3.open-usage",
];

test("T3ActionExecutor validates deep links, copies paths/messages, and fails closed", async () => {
  const { T3ActionExecutor, validT3Identifier } = await import(
    "../../plugin/streamdeck/src/t3-action-executor.ts"
  );

  const home = mkdtempSync(join(tmpdir(), "t3-home-"));
  mkdirSync(join(home, "userdata"), { recursive: true });
  writeFileSync(join(home, "userdata", "environment-id"), "env-1234-abcd\n");

  const opened = [];
  const copied = [];
  const readCalls = [];
  const executor = new T3ActionExecutor(
    home,
    async (url) => { opened.push(url); },
    async (threadId, role) => {
      readCalls.push({ threadId, role });
      return role === "user" ? "latest prompt" : "latest answer";
    },
    async (text) => { copied.push(text); },
  );

  await executor.execute("task.open", {
    selectedTask: { task_id: "t3:013c7e24-7030-46f3-ae38-282c89051064" },
  });
  await executor.execute("task.open", {
    selectedTask: { task_id: "t3:bad&calc.exe" },
  });
  await executor.execute("t3.new-thread", { selectedTask: null });
  assert.deepEqual(opened, [
    "t3code://threads/env-1234-abcd/013c7e24-7030-46f3-ae38-282c89051064",
    "t3code://",
    "t3code://new",
  ]);

  await executor.execute("task.copy-path", {
    selectedTask: { task_id: "t3:thr-1", workspace_path: "D:\\programming\\micro-emu" },
  });
  await executor.execute("task.copy-prompt", {
    selectedTask: { task_id: "t3:thr-1" },
  });
  await executor.execute("task.copy-response", {
    selectedTask: { task_id: "t3:thr-1" },
  });
  assert.deepEqual(copied, [
    "D:\\programming\\micro-emu",
    "latest prompt",
    "latest answer",
  ]);
  assert.deepEqual(readCalls, [
    { threadId: "thr-1", role: "user" },
    { threadId: "thr-1", role: "assistant" },
  ]);

  // Missing environment-id falls back cleanly to t3code://
  const emptyHome = mkdtempSync(join(tmpdir(), "t3-empty-"));
  const openedFallback = [];
  const fallbackExecutor = new T3ActionExecutor(
    emptyHome,
    async (url) => { openedFallback.push(url); },
  );
  await fallbackExecutor.execute("task.open", {
    selectedTask: { task_id: "t3:thr-1" },
  });
  assert.deepEqual(openedFallback, ["t3code://"]);

  await assert.rejects(
    executor.execute("task.fork", { selectedTask: { task_id: "t3:thr-1" } }),
    /not sent to Codex/,
  );

  assert.equal(validT3Identifier("013c7e24-7030"), "013c7e24-7030");
  assert.equal(validT3Identifier("bad/path"), null);
  assert.equal(validT3Identifier(""), null);
});

test("T3-owned tasks route to T3ActionExecutor and display provider harness labels", () => {
  assert.match(contextSource, /taskId\?\.startsWith\("t3:"\) \? "t3"/);
  assert.match(contextSource, /owner === "t3"\) \{\s*await this\.t3\.execute/);
  assert.match(contextSource, /merged\.agent = nonEmptyText\(card\.agent_label\) \?\? nonEmptyText\(card\.agent\)/);
});

test("Task Card and Knob Strip images scale long T3 provider harness labels cleanly", () => {
  const imagesSource = readFileSync(
    new URL("../../plugin/streamdeck/src/images.ts", import.meta.url),
    "utf8",
  );
  assert.match(imagesSource, /const agentLabel = agent \? truncate\(agent\.toUpperCase\(\), 15\) : ""/);
  assert.match(imagesSource, /const agentFontSize = agentLabel\.length <= 8 \? 14 : agentLabel\.length <= 11 \? 12 : 10/);
  assert.match(imagesSource, /font-size="\$\{agentFontSize\}"[^>]*>\$\{escapeXml\(agentLabel\)\}/);
  assert.match(imagesSource, /const agent = truncate\(\(ctx\.agent \?\? agentFromTaskId\(ctx\.task_id\) \?\? "—"\)\.toUpperCase\(\), 15\)/);
});

test("all 10 custom T3 Code actions are wired across catalog, Property Inspectors, and bridge", () => {
  assert.match(taskHtml, /<option value="t3">T3 Code<\/option>/);
  for (const id of T3_ACTION_IDS) {
    assert.ok(catalogSource.includes(`id: "${id}"`), `catalog missing ${id}`);
    assert.ok(actionHtml.includes(`value="${id}"`), `action.html missing ${id}`);
    assert.ok(cruxHHtml.includes(`value="${id}"`), `crux-h.html missing ${id}`);
    assert.ok(cruxVHtml.includes(`value="${id}"`), `crux-v.html missing ${id}`);
    assert.ok(codexSource.includes(`"${id}"`), `codex.rs missing ${id}`);
  }
  assert.match(bridgeSource, /CatalogAction::T3CommandPalette[\s\S]*t3_window::toggle_command_palette/);
  assert.match(bridgeSource, /CatalogAction::T3SettleThread[\s\S]*t3_window::settle_thread/);
  assert.match(bridgeSource, /CatalogAction::T3OpenUsage[\s\S]*t3_window::open_usage/);
});

test("bridge wires T3 dictation, window toggle, deep-link selection, provider-aware usage, and equal-priority slots", () => {
  const dictationHelper = bridgeSource.match(/fn route_desktop_dictation\(pressed: bool\) -> bool \{[\s\S]*?\n\}/);
  assert.ok(dictationHelper !== null);
  assert.match(
    dictationHelper[0],
    /t3_window::microphone_active\(\)\s*\|\|\s*crate::t3_window::is_foreground\(\)\.unwrap_or\(false\)[\s\S]*t3_window::set_microphone\(pressed\)/,
  );
  assert.match(
    bridgeSource,
    /AgentId::T3 => match crate::t3_window::is_foreground\(\)[\s\S]*t3_window::minimize\(\)[\s\S]*t3_window::show_and_focus\(\)/,
  );
  assert.match(
    bridgeSource,
    /owner_agent == crate::routing::AgentId::T3[\s\S]*owner_session == crate::daemon::T3_POLL_SESSION[\s\S]*t3_window::request_session_selection\(&task\.task_id\)/,
  );
  assert.match(
    pluginControllerSource,
    /t3_window::is_foreground\(\)\.unwrap_or\(false\)[\s\S]*t3_window::start_new_session\(\)/,
  );
  assert.match(usageSource, /pub\(crate\) fn from_task\(agent: crate::routing::AgentId, provider: Option<&str>\) -> Option<Self>/);
  assert.match(daemonSource, /fn task_slot_owners\(\s*slot_count: usize,\s*overrides: Option<Vec<\(usize, AgentId\)>>,\s*\) -> Vec<Option<AgentId>>/);
});
