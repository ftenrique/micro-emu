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
const modelCycleScript = readFileSync(
  new URL("../../tools/rp2040-bridge/src/cycle_codex_model.ps1", import.meta.url),
  "utf8",
);

test("Codex search and terminal catalog actions invoke desktop shortcuts", () => {
  assert.match(bridgeSource, /CatalogAction::AgentSearch[\s\S]*codex_window::search_tasks\(\)/);
  assert.match(bridgeSource, /CatalogAction::AgentOpenTerminal[\s\S]*codex_window::toggle_terminal\(\)/);
  assert.match(windowSource, /pub fn search_tasks\(\)[\s\S]*send_control_shortcut\(VK_G\)/);
  assert.match(windowSource, /pub fn toggle_terminal\(\)[\s\S]*send_control_shortcut\(VK_OEM_3\)/);
});

test("the Mic action uses Windows dictation without RP2040 serial", () => {
  assert.match(
    bridgeSource,
    /PhysicalEvent::EncoderButton \{ index: 2, pressed \}[\s\S]*!bridge\.has_serial\(\)[\s\S]*codex_window::set_microphone\(pressed\)/,
  );
  assert.match(
    windowSource,
    /pub fn set_microphone\(pressed: bool\)[\s\S]*focus_composer\(target\)\?[\s\S]*key_down\(VK_LWIN\)[\s\S]*tap_key\(VK_H\)[\s\S]*key_up\(VK_LWIN\)[\s\S]*tap_key\(VK_ESCAPE\)/,
  );
  assert.match(windowSource, /include_str!\("focus_agent_composer\.ps1"\)/);
});

test("model cycling invokes the semantic Codex UI controls without key presses", () => {
  assert.match(windowSource, /include_str!\("cycle_codex_model\.ps1"\)/);
  assert.match(windowSource, /select_next_model_with_uia\(window\)/);
  assert.match(modelCycleScript, /ExpandCollapsePattern/);
  assert.match(modelCycleScript, /InvokePattern/);
  assert.match(modelCycleScript, /Codex did not confirm model 5\.6/);
  assert.doesNotMatch(modelCycleScript, /keybd_event|SendKeys|WScript\.Shell/);
});
