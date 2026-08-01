#include "usb_descriptors.h"

#include <string.h>
#include "tusb.h"

#define USB_VENDOR_ID 0x303Au
#define USB_PRODUCT_ID 0x8360u
#define USB_DEVICE_BCD 0x0100u

#define ENDPOINT_VENDOR_HID_OUT 0x01u
#define ENDPOINT_VENDOR_HID_IN 0x81u
#define ENDPOINT_KEYBOARD_HID_OUT 0x04u
#define ENDPOINT_KEYBOARD_HID_IN 0x84u
#define ENDPOINT_CDC_NOTIFICATION 0x82u
#define ENDPOINT_CDC_OUT 0x03u
#define ENDPOINT_CDC_IN 0x83u

#define CONFIGURATION_TOTAL_LENGTH \
    (TUD_CONFIG_DESC_LEN + 2u * TUD_HID_INOUT_DESC_LEN + TUD_CDC_DESC_LEN)

/* Vendor-only descriptor (FF00:0001, Report ID 6, 49 bytes). */
uint8_t const codex_micro_report_descriptor[] = {
    0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x06, 0x09, 0x02, 0x15,
    0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x3F, 0x81, 0x02, 0x09, 0x03,
    0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x3F, 0x91, 0x02, 0x09,
    0x04, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x3F, 0xB1, 0x02,
    0xC0,
};
_Static_assert(
    sizeof(codex_micro_report_descriptor) == 49u,
    "Codex Micro vendor report descriptor must remain 49 bytes");

uint16_t const codex_micro_report_descriptor_length =
    (uint16_t)sizeof(codex_micro_report_descriptor);

/* Keyboard/consumer/mouse descriptor (Report IDs 1, 2, 3, 167 bytes).
   Stripped from the 216-byte BLE capture — everything except the vendor
   collection.  This goes on a separate interface so the Windows keyboard
   class driver (kbdhid.sys) does not lock the vendor collection. */
uint8_t const keyboard_report_descriptor[] = {
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01, 0x05, 0x07, 0x19, 0xE0,
    0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
    0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01, 0x05, 0x08,
    0x19, 0x01, 0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01,
    0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0xA4, 0x05, 0x07, 0x19, 0x00,
    0x29, 0xA4, 0x81, 0x00, 0xC0, 0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85,
    0x02, 0x75, 0x10, 0x95, 0x01, 0x15, 0x00, 0x26, 0xFF, 0x07, 0x19, 0x00,
    0x2A, 0xFF, 0x07, 0x81, 0x00, 0xC0, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x03,
    0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00,
    0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x03,
    0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F,
    0x95, 0x02, 0x75, 0x08, 0x81, 0x06, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F,
    0x95, 0x01, 0x75, 0x08, 0x81, 0x06, 0x05, 0x0C, 0x0A, 0x38, 0x02, 0x15,
    0x81, 0x25, 0x7F, 0x95, 0x01, 0x75, 0x08, 0x81, 0x06, 0xC0, 0xC0,
};
_Static_assert(
    sizeof(keyboard_report_descriptor) == 167u,
    "Keyboard report descriptor must remain 167 bytes");

uint16_t const keyboard_report_descriptor_length =
    (uint16_t)sizeof(keyboard_report_descriptor);

tusb_desc_device_t const device_descriptor = {
    .bLength = sizeof(tusb_desc_device_t),
    .bDescriptorType = TUSB_DESC_DEVICE,
    .bcdUSB = 0x0200,
    .bDeviceClass = TUSB_CLASS_MISC,
    .bDeviceSubClass = MISC_SUBCLASS_COMMON,
    .bDeviceProtocol = MISC_PROTOCOL_IAD,
    .bMaxPacketSize0 = CFG_TUD_ENDPOINT0_SIZE,
    .idVendor = USB_VENDOR_ID,
    .idProduct = USB_PRODUCT_ID,
    .bcdDevice = USB_DEVICE_BCD,
    .iManufacturer = 1,
    .iProduct = 2,
    .iSerialNumber = 3,
    .bNumConfigurations = 1,
};

uint8_t const configuration_descriptor[] = {
    TUD_CONFIG_DESCRIPTOR(
        1,
        INTERFACE_COUNT,
        0,
        CONFIGURATION_TOTAL_LENGTH,
        TUSB_DESC_CONFIG_ATT_REMOTE_WAKEUP,
        100),

    /* Interface 0: vendor HID (FF00:0001, Report ID 6).
       ChatGPT opens this — no keyboard driver locks it. */
    TUD_HID_INOUT_DESCRIPTOR(
        INTERFACE_VENDOR_HID,
        4,
        HID_ITF_PROTOCOL_NONE,
        sizeof(codex_micro_report_descriptor),
        ENDPOINT_VENDOR_HID_OUT,
        ENDPOINT_VENDOR_HID_IN,
        CFG_TUD_HID_EP_BUFSIZE,
        1),

    /* Interface 1: keyboard/consumer/mouse HID (Report IDs 1, 2, 3).
       Windows loads kbdhid.sys here, leaving the vendor interface free. */
    TUD_HID_INOUT_DESCRIPTOR(
        INTERFACE_KEYBOARD_HID,
        4,
        HID_ITF_PROTOCOL_NONE,
        sizeof(keyboard_report_descriptor),
        ENDPOINT_KEYBOARD_HID_OUT,
        ENDPOINT_KEYBOARD_HID_IN,
        CFG_TUD_HID_EP_BUFSIZE,
        1),

    TUD_CDC_DESCRIPTOR(
        INTERFACE_BRIDGE_CDC_CONTROL,
        5,
        ENDPOINT_CDC_NOTIFICATION,
        8,
        ENDPOINT_CDC_OUT,
        ENDPOINT_CDC_IN,
        CFG_TUD_CDC_EP_BUFSIZE),
};
_Static_assert(
    sizeof(configuration_descriptor) == CONFIGURATION_TOTAL_LENGTH,
    "USB configuration descriptor length mismatch");

uint8_t const *tud_descriptor_device_cb(void)
{
    return (uint8_t const *)&device_descriptor;
}

uint8_t const *tud_descriptor_configuration_cb(uint8_t index)
{
    (void)index;
    return configuration_descriptor;
}

uint8_t const *tud_hid_descriptor_report_cb(uint8_t instance)
{
    if (instance == INTERFACE_VENDOR_HID) {
        return codex_micro_report_descriptor;
    }
    return keyboard_report_descriptor;
}

static char const *string_descriptors[] = {
    (const char[]){0x09, 0x04},
    "Work Louder",
    "Codex Micro",
    "MICROEMU-RP2040-03",
    "Codex Micro HID",
    "micro-emu bridge",
};

static uint16_t string_buffer[32];

uint16_t const *tud_descriptor_string_cb(uint8_t index, uint16_t language_id)
{
    (void)language_id;
    uint8_t count;

    if (index == 0) {
        memcpy(&string_buffer[1], string_descriptors[0], 2);
        count = 1;
    } else {
        if (index >= sizeof(string_descriptors) / sizeof(string_descriptors[0])) {
            return NULL;
        }
        char const *value = string_descriptors[index];
        count = (uint8_t)strlen(value);
        if (count > 31u) {
            count = 31u;
        }
        for (uint8_t offset = 0; offset < count; ++offset) {
            string_buffer[1 + offset] = (uint8_t)value[offset];
        }
    }

    string_buffer[0] =
        (uint16_t)((TUSB_DESC_STRING << 8u) | (2u * count + 2u));
    return string_buffer;
}
