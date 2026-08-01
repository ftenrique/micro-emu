import { writeFileSync } from "node:fs";

import { CODEX_MICRO_USB_REPORT_DESCRIPTOR } from "../protocol/index.js";

const lines = [];
for (let offset = 0; offset < CODEX_MICRO_USB_REPORT_DESCRIPTOR.length; offset += 12) {
  const bytes = CODEX_MICRO_USB_REPORT_DESCRIPTOR.subarray(offset, offset + 12);
  lines.push(
    `${[...bytes]
      .map((byte) => `0x${byte.toString(16).padStart(2, "0").toUpperCase()}`)
      .join(", ")},`,
  );
}

writeFileSync(
  new URL(
    "../firmware/rp2040-zero/src/codex_micro_descriptor.inc",
    import.meta.url,
  ),
  `${lines.join("\n")}\n`,
);
