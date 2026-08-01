import {
  CODEX_MICRO_DESCRIPTOR_METADATA,
  CODEX_MICRO_REPORT_DESCRIPTOR,
  assertCodexMicroVendorReport,
} from "../protocol/index.js";

const inspection = assertCodexMicroVendorReport(
  CODEX_MICRO_REPORT_DESCRIPTOR,
);
const vendor = inspection.reports["6"];

console.log(
  JSON.stringify(
    {
      byteLength: inspection.byteLength,
      sourceTransport: CODEX_MICRO_DESCRIPTOR_METADATA.sourceTransport,
      usbDescriptorComplete:
        CODEX_MICRO_DESCRIPTOR_METADATA.usbDescriptorComplete,
      reportId6: {
        inputBytes: vendor.input[0].reportBits / 8,
        outputBytes: vendor.output[0].reportBits / 8,
        featureBytes: vendor.feature[0].reportBits / 8,
      },
      warning: CODEX_MICRO_DESCRIPTOR_METADATA.warning,
    },
    null,
    2,
  ),
);
