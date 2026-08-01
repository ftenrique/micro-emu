import { ProtocolError } from "../errors.js";

const MAIN_TAGS = new Map([
  [0x8, "input"],
  [0x9, "output"],
  [0xb, "feature"],
]);

export function inspectReportDescriptor(descriptor) {
  if (!(descriptor instanceof Uint8Array)) {
    throw new ProtocolError(
      "INVALID_DESCRIPTOR",
      "Descriptor must be a Uint8Array.",
    );
  }

  const state = {
    usagePage: 0,
    reportSize: 0,
    reportId: 0,
    reportCount: 0,
  };
  let usages = [];
  const items = [];

  for (let offset = 0; offset < descriptor.length; ) {
    const prefix = descriptor[offset];
    if (prefix === 0xfe) {
      if (offset + 2 >= descriptor.length) {
        throw new ProtocolError(
          "TRUNCATED_DESCRIPTOR_ITEM",
          `Truncated long HID item at byte ${offset}.`,
        );
      }
      const length = descriptor[offset + 1];
      const end = offset + 3 + length;
      if (end > descriptor.length) {
        throw new ProtocolError(
          "TRUNCATED_DESCRIPTOR_ITEM",
          `Long HID item at byte ${offset} exceeds descriptor length.`,
        );
      }
      offset = end;
      continue;
    }

    const sizeCode = prefix & 0x03;
    const size = sizeCode === 3 ? 4 : sizeCode;
    const type = (prefix >> 2) & 0x03;
    const tag = (prefix >> 4) & 0x0f;
    const dataStart = offset + 1;
    const end = dataStart + size;
    if (end > descriptor.length) {
      throw new ProtocolError(
        "TRUNCATED_DESCRIPTOR_ITEM",
        `HID item at byte ${offset} exceeds descriptor length.`,
      );
    }
    const value = readUnsignedLittleEndian(
      descriptor.subarray(dataStart, end),
    );

    if (type === 1) {
      if (tag === 0x0) state.usagePage = value;
      if (tag === 0x7) state.reportSize = value;
      if (tag === 0x8) state.reportId = value;
      if (tag === 0x9) state.reportCount = value;
    } else if (type === 2 && tag === 0x0) {
      usages.push(value);
    } else if (type === 0) {
      const kind = MAIN_TAGS.get(tag);
      if (kind) {
        items.push({
          kind,
          reportId: state.reportId,
          usagePage: state.usagePage,
          usage: usages.at(-1),
          reportSize: state.reportSize,
          reportCount: state.reportCount,
          reportBits: state.reportSize * state.reportCount,
          flags: value,
          offset,
        });
      }
      usages = [];
    }
    offset = end;
  }

  const reports = {};
  for (const item of items) {
    const key = String(item.reportId);
    reports[key] ??= { input: [], output: [], feature: [] };
    reports[key][item.kind].push(item);
  }

  return { byteLength: descriptor.length, items, reports };
}

export function assertCodexMicroVendorReport(descriptor) {
  const inspection = inspectReportDescriptor(descriptor);
  const report = inspection.reports["6"];
  if (!report) {
    throw new ProtocolError(
      "MISSING_VENDOR_REPORT",
      "Descriptor does not define Report ID 6.",
    );
  }
  for (const [kind, expectedUsage] of [
    ["input", 0x02],
    ["output", 0x03],
    ["feature", 0x04],
  ]) {
    const match = report[kind].find(
      (item) =>
        item.usagePage === 0xff00 &&
        item.usage === expectedUsage &&
        item.reportSize === 8 &&
        item.reportCount === 63,
    );
    if (!match) {
      throw new ProtocolError(
        "INVALID_VENDOR_REPORT",
        `Report ID 6 lacks the expected 63-byte ${kind} item.`,
      );
    }
  }
  return inspection;
}

function readUnsignedLittleEndian(bytes) {
  let value = 0;
  for (let index = 0; index < bytes.length; index += 1) {
    value += bytes[index] * 2 ** (8 * index);
  }
  return value;
}
