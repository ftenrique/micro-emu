import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  CODEX_MICRO_DESCRIPTOR_METADATA,
  CODEX_MICRO_REPORT_DESCRIPTOR,
  CODEX_MICRO_REPORT_DESCRIPTOR_HEX,
  CODEX_MICRO_USB_DESCRIPTOR_METADATA,
  CODEX_MICRO_USB_REPORT_DESCRIPTOR,
  CODEX_MICRO_USB_REPORT_DESCRIPTOR_HEX,
  assertCodexMicroVendorReport,
  inspectReportDescriptor,
} from "../../protocol/index.js";

test("published BLE descriptor bytes and metadata stay in sync", () => {
  assert.equal(CODEX_MICRO_REPORT_DESCRIPTOR.length, 216);
  assert.equal(CODEX_MICRO_REPORT_DESCRIPTOR_HEX.length, 216 * 2);
  assert.equal(CODEX_MICRO_DESCRIPTOR_METADATA.byteLength, 216);
  assert.equal(CODEX_MICRO_DESCRIPTOR_METADATA.usbDescriptorComplete, false);
});

test("USB vendor-only descriptor bytes and metadata stay in sync", () => {
  assert.equal(CODEX_MICRO_USB_REPORT_DESCRIPTOR.length, 49);
  assert.equal(CODEX_MICRO_USB_REPORT_DESCRIPTOR_HEX.length, 49 * 2);
  assert.equal(CODEX_MICRO_USB_DESCRIPTOR_METADATA.byteLength, 49);
});

test("RP2040 firmware embeds the USB vendor-only descriptor bytes", () => {
  const source = readFileSync(
    new URL(
      "../../firmware/rp2040-zero/src/usb_descriptors.c",
      import.meta.url,
    ),
    "utf8",
  );
  const vendorMatch = source.match(
    /codex_micro_report_descriptor\[\]\s*=\s*\{(?<bytes>[\s\S]*?)\};/,
  );
  assert.ok(vendorMatch?.groups?.bytes);
  const firmwareBytes = Uint8Array.from(
    [...vendorMatch.groups.bytes.matchAll(/0x([0-9A-F]{2})/gi)].map(
      (match) => Number.parseInt(match[1], 16),
    ),
  );
  assert.deepEqual(firmwareBytes, CODEX_MICRO_USB_REPORT_DESCRIPTOR);
});

test("BLE descriptor defines Report ID 6 input/output/feature at 63 bytes", () => {
  const inspection = assertCodexMicroVendorReport(
    CODEX_MICRO_REPORT_DESCRIPTOR,
  );
  const report = inspection.reports["6"];
  for (const kind of ["input", "output", "feature"]) {
    assert.equal(report[kind].length, 1);
    assert.equal(report[kind][0].usagePage, 0xff00);
    assert.equal(report[kind][0].reportBits, 63 * 8);
  }
  assert.deepEqual(
    [
      report.input[0].usage,
      report.output[0].usage,
      report.feature[0].usage,
    ],
    [2, 3, 4],
  );
});

test("USB descriptor defines Report ID 6 input/output/feature at 63 bytes", () => {
  const inspection = assertCodexMicroVendorReport(
    CODEX_MICRO_USB_REPORT_DESCRIPTOR,
  );
  const report = inspection.reports["6"];
  for (const kind of ["input", "output", "feature"]) {
    assert.equal(report[kind].length, 1);
    assert.equal(report[kind][0].usagePage, 0xff00);
    assert.equal(report[kind][0].reportBits, 63 * 8);
  }
  assert.deepEqual(
    [
      report.input[0].usage,
      report.output[0].usage,
      report.feature[0].usage,
    ],
    [2, 3, 4],
  );
});

test("descriptor parser rejects a truncated HID item", () => {
  assert.throws(
    () => inspectReportDescriptor(Uint8Array.from([0x06, 0x00])),
    (error) => error.code === "TRUNCATED_DESCRIPTOR_ITEM",
  );
});

test("Codex validator rejects a descriptor without Report ID 6", () => {
  assert.throws(
    () => assertCodexMicroVendorReport(Uint8Array.from([0x05, 0x01])),
    (error) => error.code === "MISSING_VENDOR_REPORT",
  );
});
