import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const manifest = readFileSync(new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/manifest.json", import.meta.url), "utf8");
const inspector = readFileSync(new URL("../../plugin/streamdeck/com.micro-emu.codex.sdPlugin/ui/context.html", import.meta.url), "utf8");
const action = readFileSync(new URL("../../plugin/streamdeck/src/actions/context.ts", import.meta.url), "utf8");
const images = readFileSync(new URL("../../plugin/streamdeck/src/images.ts", import.meta.url), "utf8");
const plugin = readFileSync(new URL("../../plugin/streamdeck/src/plugin.ts", import.meta.url), "utf8");
const context = readFileSync(new URL("../../plugin/streamdeck/src/context.ts", import.meta.url), "utf8");

test("Context is a Keypad action with fixed task/model/usage modes", () => {
  assert.match(manifest, /"UUID": "com\.micro-emu\.codex\.context"[\s\S]*"Controllers": \[\s*"Keypad"\s*\]/);
  assert.match(manifest, /"PropertyInspectorPath": "ui\/context\.html"/);
  assert.match(inspector, /setting="mode"/);
  for (const mode of ["task", "model", "usage"]) assert.match(inspector, new RegExp(`value="${mode}"`));
  assert.match(plugin, /new ContextKeyAction\(ctx\)/);
});

test("Context actions dispatch task search/model cycle and toggle usage reset details", () => {
  assert.match(action, /mode === "usage"/);
  assert.match(action, /showResetTimes\.set/);
  assert.match(action, /renderContextKeyImage\(mode, this\.ctx\.getSelectedDisplayContext\(\), this\.ctx\.isConnected\(\),/);
  assert.match(action, /sendCatalogAction\("agent\.search"\)/);
  assert.match(action, /sendModelCycle\(\)/);
  assert.match(action, /value === "model" \|\| value === "usage"/);
  assert.match(action, /showAlert\(\)/);
  assert.match(action, /action\.setTitle\(""\)/);
});

test("Context rendering covers strip parity and selected-task overlay", () => {
  assert.match(images, /export function renderContextKeyImage/);
  assert.match(images, /renderContextTaskBody/);
  assert.match(images, /renderContextModelBody/);
  assert.match(images, /renderContextUsageBody/);
  assert.match(images, /five_hour_reset_at/);
  assert.match(images, /weekly_reset_at/);
  assert.match(images, /formatResetAt/);
  assert.match(images, /pct <= 10/);
  assert.match(images, /pct <= 25/);
  assert.match(context, /getSelectedDisplayContext/);
  assert.match(context, /merged\.task_number = sourceSlot \+ 1/);
  assert.match(readFileSync(new URL("../../plugin/streamdeck/src/actions/crux-vertical.ts", import.meta.url), "utf8"), /onTouchTap/);
});
