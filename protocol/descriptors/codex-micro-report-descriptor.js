const descriptorHex = [
  "05010906a1018501050719e029e71500250175019508810295017508810195057501",
  "050819012905910295017503910195067508150025a40507190029a48100c0050c09",
  "01a101850275109501150026ff0719002aff078100c00902a10185030901a1000509",
  "19012905150025019505750181029501750381010501093009311581257f95027508",
  "810609381581257f950175088106050c0a38021581257f950175088106c0c00600ff",
  "0901a10185060902150026ff007508953f81020903150026ff007508953f91020904",
  "150026ff007508953fb102c0",
].join("");

export const CODEX_MICRO_REPORT_DESCRIPTOR_HEX = descriptorHex;

export const CODEX_MICRO_REPORT_DESCRIPTOR = Uint8Array.from(
  descriptorHex.match(/../g).map((byte) => Number.parseInt(byte, 16)),
);

export const CODEX_MICRO_DESCRIPTOR_METADATA = Object.freeze({
  byteLength: 216,
  source: "FreeMicro docs/PROTOCOL.md",
  sourceTransport: "Bluetooth Low Energy capture",
  observedFirmware: "0.4.1",
  observedDate: "2026-07-26",
  usbDescriptorReportedByteLength: 275,
  usbDescriptorComplete: false,
  warning:
    "The published capture is 216 bytes. Do not claim byte-for-byte USB identity until the reported 275-byte USB descriptor is captured.",
});

// Descriptor USB para el RP2040: sólo la colección vendor (Report ID 6,
// Usage Page FF00:0001). El AKP03E real separa keyboard/consumer/mouse
// en MI_01 y deja sólo vendor en MI_00. El descriptor BLE de 216 bytes
// incluye todas las colecciones en una sola interfaz, lo que hace que
// ChatGPT trate el dispositivo como teclado en vez de Codex Micro.
const usbVendorOnlyHex =
  "0600ff0901a10185060902150026ff007508953f81020903150026ff007508953f91020904150026ff007508953fb102c0";

export const CODEX_MICRO_USB_REPORT_DESCRIPTOR_HEX = usbVendorOnlyHex;

export const CODEX_MICRO_USB_REPORT_DESCRIPTOR = Uint8Array.from(
  usbVendorOnlyHex.match(/../g).map((byte) => Number.parseInt(byte, 16)),
);

export const CODEX_MICRO_USB_DESCRIPTOR_METADATA = Object.freeze({
  byteLength: 49,
  source: "Stripped from BLE capture; vendor collection only for USB MI_00",
  baseDescriptor: "CODEX_MICRO_REPORT_DESCRIPTOR (216 bytes BLE)",
  rationale:
    "The AKP03E exposes only the vendor collection (FF00:0001, Report ID 6) " +
    "on USB MI_00. Keyboard/consumer/mouse are on MI_01. Including all " +
    "collections in MI_00 causes ChatGPT to treat the device as a keyboard.",
});
