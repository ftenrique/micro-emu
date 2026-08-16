import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const pluginControllerSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/plugin_controller.rs", import.meta.url),
  "utf8",
);
const zcodeWindowSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/zcode_window.rs", import.meta.url),
  "utf8",
);
const newTaskScript = readFileSync(
  new URL("../../tools/rp2040-bridge/src/new_zcode_task.ps1", import.meta.url),
  "utf8",
);
const selectSessionScript = readFileSync(
  new URL("../../tools/rp2040-bridge/src/select_zcode_session.ps1", import.meta.url),
  "utf8",
);
const daemonClientSource = readFileSync(
  new URL("../../plugin/streamdeck/src/daemon-client.ts", import.meta.url),
  "utf8",
);
const contextSource = readFileSync(
  new URL("../../plugin/streamdeck/src/context.ts", import.meta.url),
  "utf8",
);

test("the daemon handles new-task requests only while ZCode is foreground", () => {
  assert.match(pluginControllerSource, /"new-task" => \{[\s\S]*zcode_window::is_foreground\(\)\.unwrap_or\(false\)/);
  assert.match(pluginControllerSource, /"new-task" => \{[\s\S]*zcode_window::request_new_task\(\)/);
  assert.match(pluginControllerSource, /"type":"new-task-result", "handled": handled/);
});

test("new task creation shares the serialized sidebar automation queue", () => {
  assert.match(zcodeWindowSource, /include_str!\("new_zcode_task\.ps1"\)/);
  assert.match(zcodeWindowSource, /pub fn request_new_task\(\)/);
  assert.match(zcodeWindowSource, /AutomationRequest::NewTask/);
  // Selection and new task funnel into one worker so their sidebar toggles
  // can never interleave.
  assert.match(zcodeWindowSource, /fn queue_request\(request: AutomationRequest\)/);
  assert.doesNotMatch(zcodeWindowSource, /SELECTION_QUEUE/);
});

test("the new task script drives only the Tasks-section button", () => {
  // Project rows also carry "New task" buttons; the neutral one is the
  // button whose control-view parent group is named "Tasks".
  assert.match(newTaskScript, /ControlType\]::Button/);
  assert.match(newTaskScript, /\$element\.Current\.Name -ne "New task"/);
  assert.match(newTaskScript, /GetParent\(\$element\)/);
  assert.match(newTaskScript, /\$parent\.Current\.Name -eq "Tasks"/);
  // Same invocation contract as session selection: invoke pattern only,
  // collapsed sidebar opened and restored, success marker on stdout.
  assert.match(newTaskScript, /InvokePattern/);
  assert.match(newTaskScript, /"Toggle sidebar"/);
  assert.match(newTaskScript, /\$sidebarOpened = \$true/);
  assert.match(newTaskScript, /"created"/);
  assert.doesNotMatch(newTaskScript, /keybd_event|SendKeys|WScript\.Shell/);
});

test("the MSAA poke passes OBJID_CLIENT as an unsigned value", () => {
  // PowerShell evaluates 0xFFFFFFF0 as a negative int that cannot convert to
  // the uint P/Invoke parameter, so the poke must use an explicit uint cast.
  for (const script of [newTaskScript, selectSessionScript]) {
    assert.doesNotMatch(script, /AccessibleObjectFromWindow\([^,]+, 0xFFFFFFF0/);
    assert.match(script, /AccessibleObjectFromWindow\([^,]+, \[uint32\]4294967280/);
  }
});

test("the plugin falls back to the Codex screen when ZCode does not handle it", () => {
  assert.match(daemonClientSource, /requestZcodeNewTask\(timeoutMs = 3_000\): Promise<boolean>/);
  assert.match(daemonClientSource, /type: "new-task"/);
  assert.match(daemonClientSource, /type === "new-task-result"/);
  assert.match(daemonClientSource, /failNewTaskWaiters\(\)/);
  // The interception must run before the selected task's agent routing;
  // otherwise a selected ZCode card would reject the action.
  const interceptAt = contextSource.indexOf('actionId === "agent.new-task"');
  const ownerRoutingAt = contextSource.indexOf('if (owner === "zcode")');
  assert.ok(interceptAt >= 0, "context must intercept agent.new-task");
  assert.ok(interceptAt < ownerRoutingAt, "the interception must precede agent routing");
  assert.match(contextSource, /requestZcodeNewTask\(\)[\s\S]*await this\.codex\.execute\(actionId/);
});
