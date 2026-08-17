import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const bridgeSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/main.rs", import.meta.url),
  "utf8",
);
const hermesWindowSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/hermes_window.rs", import.meta.url),
  "utf8",
);
const hermesSelectScript = readFileSync(
  new URL("../../tools/rp2040-bridge/src/hermes_select_session.ps1", import.meta.url),
  "utf8",
);
const daemonSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/daemon.rs", import.meta.url),
  "utf8",
);
const pluginControllerSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/plugin_controller.rs", import.meta.url),
  "utf8",
);

test("new Hermes sessions are started with the Ctrl+N accelerator, focus-gated", () => {
  assert.match(hermesWindowSource, /pub fn start_new_session\(\) -> Result<\(\), String>/);
  assert.match(hermesWindowSource, /const VK_CONTROL: u8 = 0x11/);
  assert.match(hermesWindowSource, /const VK_N: u8 = 0x4E/);
  // The chord must never land in another app, so the foreground is
  // re-checked right before sending it.
  assert.match(
    hermesWindowSource,
    /pub fn start_new_session[\s\S]*GetForegroundWindow\(\) \} != target[\s\S]*key_down\(VK_CONTROL\);[\s\S]*tap_key\(VK_N\);[\s\S]*key_up\(VK_CONTROL\);/,
  );
});

test("the Hermes mic action mirrors the ZCode dictation semantics", () => {
  assert.match(hermesWindowSource, /pub fn set_microphone\(pressed: bool\) -> Result<\(\), String>/);
  assert.match(hermesWindowSource, /const VK_LWIN: u8 = 0x5B/);
  assert.match(hermesWindowSource, /const VK_ESCAPE: u8 = 0x1B/);
  assert.match(
    hermesWindowSource,
    /pub fn set_microphone[\s\S]*if pressed \{[\s\S]*GetForegroundWindow\(\) \} != target[\s\S]*key_down\(VK_LWIN\);[\s\S]*tap_key\(VK_H\);[\s\S]*key_up\(VK_LWIN\);/,
  );
  assert.match(
    hermesWindowSource,
    /pub fn set_microphone[\s\S]*\} else if DICTATION_ACTIVE[\s\S]*swap\(false[\s\S]*tap_key\(VK_ESCAPE\)/,
  );
  assert.match(hermesWindowSource, /pub fn microphone_active\(\) -> bool/);
  assert.match(hermesWindowSource, /const FOCUS_COMPOSER_SCRIPT: &str = include_str!\("focus_agent_composer\.ps1"\)/);
  assert.match(hermesWindowSource, /focus_composer\(target\)\?/);
});

test("the daemon new-task handler serves Hermes after ZCode", () => {
  assert.match(
    pluginControllerSource,
    /"new-task" => \{[\s\S]*zcode_window::is_foreground\(\)\.unwrap_or\(false\)[\s\S]*zcode_window::request_new_task\(\);[\s\S]*\} else if crate::hermes_window::is_foreground\(\)\.unwrap_or\(false\) \{[\s\S]*hermes_window::start_new_session\(\)/,
  );
  assert.match(pluginControllerSource, /"type":"new-task-result", "handled": handled/);
});

test("every encoder-button mic route offers the Hermes dictation branch", () => {
  const branches = bridgeSource.match(
    /EncoderButton \{ index: 2, pressed \} =\s*event\.clone\(\)\s*\{[\s\S]*?if !bridge\.has_serial\(\)/g,
  );
  assert.ok(branches !== null && branches.length === 2, "expected the primary and task-device mic routes");
  for (const branch of branches) {
    assert.match(branch, /zcode_window::microphone_active\(\)/);
    assert.match(branch, /hermes_window::microphone_active\(\)/);
    assert.match(branch, /hermes_window::is_foreground\(\)\.unwrap_or\(false\)/);
    assert.match(branch, /hermes_window::set_microphone\(pressed\)/);
  }
});

test("pressing an auto-fed Hermes card drives the desktop app to that session", () => {
  // The shared card-press path queues a Hermes sidebar selection for
  // auto-fed cards, mirroring the ZCode branch.
  assert.match(
    bridgeSource,
    /owner_agent == crate::routing::AgentId::Hermes[\s\S]*owner_session == crate::daemon::HERMES_POLL_SESSION[\s\S]*hermes_window::request_session_selection\(&task\.task_id, &task\.title\)/,
  );
  assert.match(
    hermesWindowSource,
    /pub fn request_session_selection\(session_id: &str, title: &str\)/,
  );
  assert.match(
    hermesWindowSource,
    /const SELECT_SESSION_SCRIPT: &str = include_str!\("hermes_select_session\.ps1"\)/,
  );
  assert.match(
    hermesWindowSource,
    /fn run_automation\(\s*script: &str,\s*success_marker: &str,\s*supersede: Option<\(u64, &'static AutomationQueue\)>,?\s*\)/,
  );
});

test("the Hermes selection script targets sidebar rows, search, and tab activation", () => {
  // Session rows are buttons whose class starts with the sidebar row
  // signature; exact names beat "Reorder "-prefixed ones.
  assert.match(hermesSelectScript, /ClassName\.StartsWith\("pl-2 pr-1 gap-1\.5"/);
  assert.match(hermesSelectScript, /if \(\$Name -ceq \$TargetTitle\) \{ return 3 \}/);
  assert.match(hermesSelectScript, /"Reorder " \+ \$TargetTitle/);
  // Older sessions hide behind pagination; the search box surfaces them and
  // is cleared afterwards.
  assert.match(hermesSelectScript, /\$element\.Current\.Name -eq "Search sessions"/);
  assert.match(hermesSelectScript, /Set-SearchText \$searchBox ""/);
  // Activation is confirmed through the active editor tab.
  assert.match(hermesSelectScript, /ControlType\]::TabItem/);
  assert.match(hermesSelectScript, /ClassName\.Contains\("tab-active"\)/);
  assert.match(hermesSelectScript, /\[Console\]::Out\.WriteLine\("selected"\)/);
});

test("the daemon keeps the Hermes feed alive while the desktop app runs", () => {
  assert.match(hermesWindowSource, /pub fn desktop_running\(\) -> bool/);
  assert.match(daemonSource, /const HERMES_DESKTOP_PROBE_INTERVAL/);
  assert.match(daemonSource, /hermes_desktop_active, false, false\);|hermes_desktop_active = crate::hermes_window::desktop_running\(\)/);
  // Auto-feed gating treats a running desktop app like a live proxy.
  assert.match(
    daemonSource,
    /let hermes_active = hermes_proxy_active \|\| hermes_desktop_active;/,
  );
  assert.match(
    daemonSource,
    /crate::hermes_window::desktop_running\(\);[\s\S]*pending_repartition_at = Some\(now \+ REPARTITION_DEBOUNCE\)/,
  );
});

test("a refused Hermes foreground activation falls back to the Electron relaunch handshake", () => {
  assert.match(hermesWindowSource, /fn relaunch_and_wait_for_focus\(target: Hwnd, exe: String\)/);
  assert.match(
    hermesWindowSource,
    /match window_process_image\(target\)[\s\S]*relaunch_and_wait_for_focus\(target, exe\)/,
  );
  // The relaunch can never start a foreign binary: the image is re-checked
  // against the Hermes executable name.
  assert.match(
    hermesWindowSource,
    /fn window_process_image\(window: Hwnd\) -> Option<String>[\s\S]*filter\(\|path\| is_hermes_desktop_executable\(path\)\)/,
  );
});
