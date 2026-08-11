import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const bridgeSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/main.rs", import.meta.url),
  "utf8",
);
const windowSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/codex_window.rs", import.meta.url),
  "utf8",
);

test("Codex search and terminal catalog actions invoke desktop shortcuts", () => {
  assert.match(bridgeSource, /CatalogAction::AgentSearch[\s\S]*codex_window::search_tasks\(\)/);
  assert.match(bridgeSource, /CatalogAction::AgentOpenTerminal[\s\S]*codex_window::toggle_terminal\(\)/);
  assert.match(windowSource, /pub fn search_tasks\(\)[\s\S]*send_control_shortcut\(VK_G\)/);
  assert.match(windowSource, /pub fn toggle_terminal\(\)[\s\S]*send_control_shortcut\(VK_OEM_3\)/);
});
