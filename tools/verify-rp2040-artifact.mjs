import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { CODEX_MICRO_USB_REPORT_DESCRIPTOR } from "../protocol/index.js";

const UF2_MAGIC_START0 = 0x0a324655;
const UF2_MAGIC_START1 = 0x9e5d5157;
const UF2_MAGIC_END = 0x0ab16f30;
const UF2_FLAG_FAMILY_ID_PRESENT = 0x00002000;
const RP2040_FAMILY_ID = 0xe48bff56;
const BLOCK_BYTES = 512;
const DATA_OFFSET = 32;
const DATA_CAPACITY = 476;
const EXPECTED_FIRMWARE_BUILD = "rp2040-zero/0.1.1-diag";

const artifactPath = resolve(
  process.argv[2] ??
    "firmware/rp2040-zero/build/codex_micro_rp2040_bridge.uf2",
);
const uf2 = readFileSync(artifactPath);
assert.equal(uf2.length % BLOCK_BYTES, 0, "UF2 size must be a multiple of 512");
const blockCount = uf2.length / BLOCK_BYTES;
assert.ok(blockCount > 0, "UF2 must contain at least one block");

const chunks = [];
for (let index = 0; index < blockCount; index += 1) {
  const block = uf2.subarray(index * BLOCK_BYTES, (index + 1) * BLOCK_BYTES);
  assert.equal(block.readUInt32LE(0), UF2_MAGIC_START0, `block ${index} magic 0`);
  assert.equal(block.readUInt32LE(4), UF2_MAGIC_START1, `block ${index} magic 1`);
  assert.equal(block.readUInt32LE(508), UF2_MAGIC_END, `block ${index} end magic`);
  const flags = block.readUInt32LE(8);
  assert.ok(
    flags & UF2_FLAG_FAMILY_ID_PRESENT,
    `block ${index} must declare a family ID`,
  );
  assert.equal(
    block.readUInt32LE(28),
    RP2040_FAMILY_ID,
    `block ${index} RP2040 family`,
  );
  const payloadSize = block.readUInt32LE(16);
  assert.ok(
    payloadSize > 0 && payloadSize <= DATA_CAPACITY,
    `block ${index} payload size`,
  );
  assert.equal(block.readUInt32LE(20), index, `block ${index} sequence`);
  assert.equal(block.readUInt32LE(24), blockCount, `block ${index} count`);
  chunks.push({
    address: block.readUInt32LE(12),
    payload: block.subarray(DATA_OFFSET, DATA_OFFSET + payloadSize),
  });
}

const baseAddress = Math.min(...chunks.map(({ address }) => address));
const endAddress = Math.max(
  ...chunks.map(({ address, payload }) => address + payload.length),
);
const flash = Buffer.alloc(endAddress - baseAddress, 0xff);
for (const { address, payload } of chunks) {
  payload.copy(flash, address - baseAddress);
}

const descriptor = Buffer.from(CODEX_MICRO_USB_REPORT_DESCRIPTOR);
assert.notEqual(
  flash.indexOf(descriptor),
  -1,
  "compiled flash image must contain the exact Codex Micro USB vendor descriptor",
);
for (const text of ["Work Louder", "Codex Micro", "micro-emu bridge"]) {
  assert.notEqual(
    flash.indexOf(Buffer.from(text, "ascii")),
    -1,
    `compiled flash image must contain ${JSON.stringify(text)}`,
  );
}
assert.notEqual(
  flash.indexOf(Buffer.from(EXPECTED_FIRMWARE_BUILD, "ascii")),
  -1,
  `compiled flash image must contain the diagnostic build ID ${JSON.stringify(EXPECTED_FIRMWARE_BUILD)}`,
);

console.log(
  JSON.stringify({
    valid: true,
    path: artifactPath,
    bytes: uf2.length,
    blocks: blockCount,
    flashStart: `0x${baseAddress.toString(16)}`,
    flashEnd: `0x${endAddress.toString(16)}`,
    sha256: createHash("sha256").update(uf2).digest("hex"),
    descriptorBytes: descriptor.length,
    descriptorTransport: "usb-vendor",
    firmwareBuild: EXPECTED_FIRMWARE_BUILD,
  }),
);
