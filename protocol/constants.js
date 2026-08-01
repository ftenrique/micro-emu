export const VENDOR_ID = 0x303a;
export const PRODUCT_ID = 0x8360;
export const USAGE_PAGE = 0xff00;
export const REPORT_ID = 6;
export const REPORT_BYTES_USB = 63;
export const REPORT_BYTES_BLE = 64;
export const OPCODE_DATA = 0x02;
export const FRAME_HEADER_BYTES = 2;
export const MAX_CHUNK_BYTES = REPORT_BYTES_USB - FRAME_HEADER_BYTES;
export const MESSAGE_TERMINATOR = new Uint8Array([0x0d, 0x0a]);

export const Transport = Object.freeze({
  USB: "usb",
  BLE: "ble",
});

export const Methods = Object.freeze({
  DEVICE_STATUS: "device.status",
  KEY_EVENT: "v.oai.hid",
  RADIAL_EVENT: "v.oai.rad",
  RGB_CONFIG: "v.oai.rgbcfg",
  THREAD_STATUS: "v.oai.thstatus",
});

export const Effects = Object.freeze({
  OFF: 0,
  SOLID: 1,
  SNAKE: 2,
  RAINBOW: 3,
  BREATH: 4,
  GRADIENT: 5,
  SHALLOW_BREATH: 6,
});
