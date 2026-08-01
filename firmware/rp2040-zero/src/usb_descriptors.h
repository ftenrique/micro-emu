#pragma once

#include <stdint.h>

enum {
    INTERFACE_VENDOR_HID = 0,
    INTERFACE_KEYBOARD_HID,
    INTERFACE_BRIDGE_CDC_CONTROL,
    INTERFACE_BRIDGE_CDC_DATA,
    INTERFACE_COUNT,
};

#define CODEX_REPORT_ID 6u
#define CODEX_REPORT_PAYLOAD_BYTES 63u
#define CODEX_REPORT_WIRE_BYTES 64u

extern uint8_t const codex_micro_report_descriptor[];
extern uint16_t const codex_micro_report_descriptor_length;
extern uint8_t const keyboard_report_descriptor[];
extern uint16_t const keyboard_report_descriptor_length;
