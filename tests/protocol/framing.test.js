import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  FrameDecoder,
  MAX_CHUNK_BYTES,
  OPCODE_DATA,
  REPORT_BYTES_BLE,
  REPORT_BYTES_USB,
  REPORT_ID,
  Transport,
  decodeReports,
  frameJson,
  frameMessage,
} from "../../protocol/index.js";

const statusFixture = JSON.parse(
  readFileSync(
    new URL(
      "../../protocol/fixtures/device-status-frames.json",
      import.meta.url,
    ),
    "utf8",
  ),
);

test("device.status framing matches the byte fixture exactly", () => {
  const usb = frameJson(statusFixture.message).map((report) =>
    Buffer.from(report).toString("hex"),
  );
  const ble = frameJson(statusFixture.message, {
    transport: Transport.BLE,
  }).map((report) => Buffer.from(report).toString("hex"));
  assert.deepEqual(usb, statusFixture.usbReportsHex);
  assert.deepEqual(ble, statusFixture.bleReportsHex);
});

test("USB framing emits an exact 63-byte report", () => {
  const [report] = frameMessage('{"m":"x"}');
  assert.equal(report.length, REPORT_BYTES_USB);
  assert.equal(report[0], OPCODE_DATA);
  assert.equal(report[1], 11);
  assert.deepEqual(
    report.subarray(2, 13),
    new TextEncoder().encode('{"m":"x"}\r\n'),
  );
  assert.ok(report.subarray(13).every((byte) => byte === 0));
});

test("BLE framing adds the Report ID prefix and preserves payload", () => {
  const [usb] = frameMessage('{"m":"x"}');
  const [ble] = frameMessage('{"m":"x"}', { transport: Transport.BLE });
  assert.equal(ble.length, REPORT_BYTES_BLE);
  assert.equal(ble[0], REPORT_ID);
  assert.deepEqual(ble.subarray(1), usb);
});

test("long UTF-8 payloads fragment and reassemble without loss", () => {
  const message = {
    m: "v.oai.thstatus",
    p: Array.from({ length: 6 }, (_, id) => ({
      id,
      label: "tarea-ñ-🙂".repeat(8),
    })),
  };
  const reports = frameJson(message);
  assert.ok(reports.length > 2);
  assert.ok(reports.every((report) => report.length === REPORT_BYTES_USB));
  assert.deepEqual(decodeReports(reports), { messages: [message], errors: [] });
});

test("an exact multiple of chunk size emits a final CRLF fragment", () => {
  const reports = frameMessage("x".repeat(MAX_CHUNK_BYTES * 2));
  assert.equal(reports.length, 3);
  assert.deepEqual(
    reports.map((report) => report[1]),
    [MAX_CHUNK_BYTES, MAX_CHUNK_BYTES, 2],
  );
});

test("decoder accepts both USB and Report-ID-prefixed reports", () => {
  const message = { m: "ping" };
  for (const transport of [Transport.USB, Transport.BLE]) {
    const result = decodeReports(frameJson(message, { transport }));
    assert.deepEqual(result.messages, [message]);
    assert.deepEqual(result.errors, []);
  }
});

test("decoder extracts concatenated messages from one report", () => {
  const body = new TextEncoder().encode('{"m":"one"}\r\n{"m":"two"}\r\n');
  const report = new Uint8Array(REPORT_BYTES_USB);
  report[0] = OPCODE_DATA;
  report[1] = body.length;
  report.set(body, 2);
  const result = new FrameDecoder().feed(report);
  assert.deepEqual(result.messages, [{ m: "one" }, { m: "two" }]);
  assert.deepEqual(result.errors, []);
});

test("incorrect report lengths are rejected without throwing", () => {
  const result = new FrameDecoder().feed(new Uint8Array(62));
  assert.deepEqual(result.messages, []);
  assert.equal(result.errors[0].code, "INVALID_REPORT_LENGTH");
});

test("wrong opcode and Report ID are rejected", () => {
  const decoder = new FrameDecoder();
  const usb = new Uint8Array(REPORT_BYTES_USB);
  const ble = new Uint8Array(REPORT_BYTES_BLE);
  ble[0] = REPORT_ID + 1;
  assert.equal(decoder.feed(usb).errors[0].code, "INVALID_REPORT_HEADER");
  assert.equal(decoder.feed(ble).errors[0].code, "INVALID_REPORT_HEADER");
});

test("declared chunk lengths over 61 are rejected", () => {
  const report = new Uint8Array(REPORT_BYTES_USB);
  report[0] = OPCODE_DATA;
  report[1] = MAX_CHUNK_BYTES + 1;
  const result = new FrameDecoder().feed(report);
  assert.equal(result.errors[0].code, "INVALID_CHUNK_LENGTH");
});

test("truncated logical data is reported by finish", () => {
  const report = new Uint8Array(REPORT_BYTES_USB);
  const body = new TextEncoder().encode('{"m":"unfinished"}');
  report[0] = OPCODE_DATA;
  report[1] = body.length;
  report.set(body, 2);
  const decoder = new FrameDecoder();
  assert.equal(decoder.feed(report).errors.length, 0);
  const final = decoder.finish();
  assert.equal(final.errors[0].code, "TRUNCATED_MESSAGE");
  assert.equal(decoder.bufferedBytes, 0);
});

test("invalid JSON is isolated and the decoder recovers", () => {
  const decoder = new FrameDecoder();
  const invalid = decoder.feed(frameMessage("not-json")[0]);
  assert.equal(invalid.errors[0].code, "INVALID_JSON");
  const valid = decoder.feed(frameMessage('{"m":"ok"}')[0]);
  assert.deepEqual(valid.messages, [{ m: "ok" }]);
});

test("buffer limit discards an unterminated oversized message", () => {
  const decoder = new FrameDecoder({ maxBufferedBytes: 70 });
  const reports = frameMessage("x".repeat(100));
  assert.equal(decoder.feed(reports[0]).errors.length, 0);
  const second = decoder.feed(reports[1]);
  assert.equal(second.errors[0].code, "BUFFER_LIMIT_EXCEEDED");
  assert.equal(decoder.bufferedBytes, 0);
});

test("frameMessage validates transport and message size", () => {
  assert.throws(
    () => frameMessage("x", { transport: "serial" }),
    (error) => error.code === "INVALID_TRANSPORT",
  );
  assert.throws(
    () => frameMessage("1234", { maxMessageBytes: 5 }),
    (error) => error.code === "MESSAGE_TOO_LARGE",
  );
});
