import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const catalogSource = readFileSync(
  new URL("../../plugin/streamdeck/src/action-catalog.ts", import.meta.url),
  "utf8",
);
const executorSource = readFileSync(
  new URL("../../plugin/streamdeck/src/codex-action-executor.ts", import.meta.url),
  "utf8",
);
const propertyInspector = readFileSync(
  new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/ui/action.html", import.meta.url),
  "utf8",
);

const retiredIds = [
  "task.interrupt",
  "task.pin",
  "task.unpin",
  "task.approve",
  "task.reject",
  "agent.open-browser",
  "agent.open-editor",
];

test("the Action Button picker contains no advertised no-op actions", () => {
  const options = [...propertyInspector.matchAll(
    /<option value="((?:micro|task|agent)\.[^"]+)"[^>]*data-executor="([^"]+)"/g,
  )].map((match) => ({ id: match[1], executor: match[2] }));

  assert.ok(options.length > 20, "expected the complete active catalog");
  assert.equal(options.some((option) => option.executor === "Agent"), false);
  assert.equal(propertyInspector.includes("agent adapter"), false);
  for (const id of retiredIds) {
    assert.equal(options.some((option) => option.id === id), false, `${id} must not be offered`);
  }
});

test("every advertised Codex action has a concrete local executor", () => {
  const codexIds = [...propertyInspector.matchAll(
    /<option value="((?:task|agent)\.[^"]+)"[^>]*data-executor="Codex"/g,
  )].map((match) => match[1]);

  assert.deepEqual(codexIds.sort(), [
    "agent.compact-context",
    "agent.new-task",
    "agent.review-changes",
    "agent.run-tests",
    "agent.settings",
    "task.archive",
    "task.copy-path",
    "task.copy-prompt",
    "task.copy-response",
    "task.fork",
    "task.open",
    "task.retry",
  ]);

  for (const id of codexIds) {
    const catalogLine = catalogSource.split(/\r?\n/).find((line) => line.includes(`id: "${id}"`));
    assert.match(catalogLine ?? "", /executor: "Codex"/);
    assert.match(catalogLine ?? "", /dispatch: \{ kind: "codex-action" \}/);
    assert.match(executorSource, new RegExp(`case "${id.replace(".", "\\.")}"|actionId === "${id.replace(".", "\\.")}"`));
  }

  for (const method of [
    "thread/read",
    "thread/resume",
    "thread/fork",
    "thread/archive",
    "turn/start",
    "review/start",
    "thread/compact/start",
  ]) {
    assert.ok(executorSource.includes(`"${method}"`), `missing Codex method ${method}`);
  }
});

test("retired profile ids resolve to an alert instead of ACT06", () => {
  for (const id of retiredIds) {
    const line = catalogSource.split(/\r?\n/).find((entry) => entry.includes(`id: "${id}"`));
    assert.match(line ?? "", /executor: "Unavailable"/);
    assert.match(line ?? "", /kind: "unsupported"/);
  }
  assert.match(catalogSource, /ACTIONS_BY_ID\.get\(actionId\) \?\? RETIRED_BY_ID\.get\(actionId\)/);
});
