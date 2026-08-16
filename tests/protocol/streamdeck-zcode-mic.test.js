import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const bridgeSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/main.rs", import.meta.url),
  "utf8",
);
const zcodeWindowSource = readFileSync(
  new URL("../../tools/rp2040-bridge/src/zcode_window.rs", import.meta.url),
  "utf8",
);

test("the ZCode mic action opens Windows dictation on press and closes it on release", () => {
  assert.match(zcodeWindowSource, /pub fn set_microphone\(pressed: bool\) -> Result<\(\), String>/);
  assert.match(zcodeWindowSource, /const VK_LWIN: u8 = 0x5B/);
  assert.match(zcodeWindowSource, /const VK_H: u8 = 0x48/);
  assert.match(zcodeWindowSource, /const VK_ESCAPE: u8 = 0x1B/);
  // Press: the chord must never land in another app, so the foreground is
  // re-checked right before sending Win+H.
  assert.match(
    zcodeWindowSource,
    /pub fn set_microphone[\s\S]*if pressed \{[\s\S]*GetForegroundWindow\(\) \} != target[\s\S]*key_down\(VK_LWIN\);[\s\S]*tap_key\(VK_H\);[\s\S]*key_up\(VK_LWIN\);/,
  );
  // Release: Escape is only sent when this bridge opened the dictation bar.
  assert.match(
    zcodeWindowSource,
    /pub fn set_microphone[\s\S]*\} else if DICTATION_ACTIVE[\s\S]*swap\(false[\s\S]*tap_key\(VK_ESCAPE\)/,
  );
  assert.match(zcodeWindowSource, /pub fn microphone_active\(\) -> bool/);
});

test("every encoder-button mic route offers the ZCode dictation branch first", () => {
  const branches = bridgeSource.match(
    /EncoderButton \{ index: 2, pressed \} =\s*event\.clone\(\)\s*\{[\s\S]*?if !bridge\.has_serial\(\)/g,
  );
  assert.ok(branches !== null && branches.length === 2, "expected the primary and task-device mic routes");
  for (const branch of branches) {
    assert.match(
      branch,
      /zcode_window::microphone_active\(\)\s*\|\|\s*crate::zcode_window::is_foreground\(\)\.unwrap_or\(false\)/,
    );
    assert.match(branch, /zcode_window::set_microphone\(pressed\)/);
  }
});
