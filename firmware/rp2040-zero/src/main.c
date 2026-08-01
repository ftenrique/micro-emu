#include <stdarg.h>
#include <stdio.h>
#include <string.h>

#include "bsp/board.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "bridge_protocol.h"
#include "usb_descriptors.h"

#define REPORT_QUEUE_CAPACITY 8u
#define BRIDGE_TX_QUEUE_CAPACITY 32u
#define FIRMWARE_BUILD_ID "rp2040-zero/0.1.1-diag"

typedef struct {
    uint8_t reports[REPORT_QUEUE_CAPACITY][CODEX_REPORT_WIRE_BYTES];
    uint8_t head;
    uint8_t count;
} report_queue_t;

typedef struct {
    bridge_frame_t frames[BRIDGE_TX_QUEUE_CAPACITY];
    uint8_t head;
    uint8_t count;
} bridge_tx_queue_t;

static bridge_parser_t bridge_parser;
static report_queue_t input_reports;
static bridge_tx_queue_t bridge_tx;
static uint16_t next_sequence = 1u;
static bool boot_log_pending = true;

static bool report_queue_push(
    report_queue_t *queue,
    uint8_t const report[CODEX_REPORT_WIRE_BYTES])
{
    if (queue->count == REPORT_QUEUE_CAPACITY) {
        return false;
    }
    uint8_t slot = (uint8_t)((queue->head + queue->count) %
        REPORT_QUEUE_CAPACITY);
    memcpy(queue->reports[slot], report, CODEX_REPORT_WIRE_BYTES);
    ++queue->count;
    return true;
}

static void report_queue_pop(report_queue_t *queue)
{
    if (queue->count == 0u) {
        return;
    }
    queue->head = (uint8_t)((queue->head + 1u) % REPORT_QUEUE_CAPACITY);
    --queue->count;
}

static bool bridge_tx_push(
    uint8_t type,
    uint8_t const *payload,
    uint16_t length)
{
    if (length > BRIDGE_MAX_PAYLOAD ||
        bridge_tx.count == BRIDGE_TX_QUEUE_CAPACITY) {
        return false;
    }
    uint8_t slot = (uint8_t)((bridge_tx.head + bridge_tx.count) %
        BRIDGE_TX_QUEUE_CAPACITY);
    bridge_frame_t *frame = &bridge_tx.frames[slot];
    frame->type = type;
    frame->sequence = next_sequence++;
    frame->length = length;
    if (length > 0u) {
        memcpy(frame->payload, payload, length);
    }
    ++bridge_tx.count;
    return true;
}

static void bridge_log(char const *format, ...)
{
    if (bridge_tx.count > BRIDGE_TX_QUEUE_CAPACITY - 4u) {
        return;
    }
    char text[BRIDGE_MAX_PAYLOAD];
    va_list arguments;
    va_start(arguments, format);
    int written = vsnprintf(text, sizeof(text), format, arguments);
    va_end(arguments);
    if (written <= 0) {
        return;
    }
    if ((size_t)written >= sizeof(text)) {
        written = (int)sizeof(text) - 1;
    }
    (void)bridge_tx_push(
        BRIDGE_FRAME_LOG,
        (uint8_t const *)text,
        (uint16_t)written);
}

static void bridge_tx_flush(void)
{
    if (bridge_tx.count == 0u) {
        return;
    }
    /* Wait until CDC is connected (DTR asserted) before flushing. */
    if (!tud_cdc_connected()) {
        return;
    }
    bridge_frame_t *frame = &bridge_tx.frames[bridge_tx.head];
    uint8_t encoded[BRIDGE_MAX_FRAME_BYTES];
    size_t length = bridge_frame_encode(frame, encoded, sizeof(encoded));
    if (length == 0u || tud_cdc_write_available() < length) {
        return;
    }
    tud_cdc_write(encoded, (uint32_t)length);
    tud_cdc_write_flush();
    bridge_tx.head =
        (uint8_t)((bridge_tx.head + 1u) % BRIDGE_TX_QUEUE_CAPACITY);
    --bridge_tx.count;
}

static void on_bridge_frame(
    bridge_frame_t const *frame,
    void *context)
{
    (void)context;
    if (frame->type == BRIDGE_FRAME_CODEX_INPUT_REPORT &&
        frame->length == CODEX_REPORT_WIRE_BYTES &&
        frame->payload[0] == CODEX_REPORT_ID) {
        if (!report_queue_push(&input_reports, frame->payload)) {
            bridge_log("in queue full seq=%u", (unsigned)frame->sequence);
        }
        return;
    }
    if (frame->type == BRIDGE_FRAME_PING) {
        static uint8_t const status[] = FIRMWARE_BUILD_ID;
        (void)bridge_tx_push(
            BRIDGE_FRAME_STATUS,
            status,
            (uint16_t)(sizeof(status) - 1u));
    }
}

static void bridge_rx_poll(void)
{
    uint8_t buffer[64];
    while (tud_cdc_available() > 0u) {
        uint32_t read = tud_cdc_read(buffer, sizeof(buffer));
        bridge_parser_feed(
            &bridge_parser,
            buffer,
            read,
            on_bridge_frame,
            NULL);
    }
}

static void hid_input_flush(void)
{
    if (input_reports.count == 0u || !tud_hid_n_ready(0)) {
        return;
    }
    uint8_t const *report = input_reports.reports[input_reports.head];
    if (!tud_hid_n_report(
            0,
            CODEX_REPORT_ID,
            &report[1],
            CODEX_REPORT_PAYLOAD_BYTES)) {
        return;
    }
    bridge_log(
        "hid in accepted id=%u chan=%u len=%u queued=%u",
        (unsigned)report[0],
        (unsigned)report[1],
        (unsigned)report[2],
        (unsigned)(input_reports.count - 1u));
    report_queue_pop(&input_reports);
}

int main(void)
{
    board_init();
    tusb_init();
    bridge_parser_init(&bridge_parser);

    while (true) {
        tud_task();
        bridge_rx_poll();
        hid_input_flush();

        /* Emit boot log once CDC is connected (DTR asserted). */
        if (boot_log_pending && tud_cdc_connected()) {
            bridge_log("boot %s usage-page=FF00", FIRMWARE_BUILD_ID);
            boot_log_pending = false;
        }

        bridge_tx_flush();
        tight_loop_contents();
    }
}

uint16_t tud_hid_get_report_cb(
    uint8_t instance,
    uint8_t report_id,
    hid_report_type_t report_type,
    uint8_t *buffer,
    uint16_t requested_length)
{
    /* Only handle the vendor HID instance (instance 0). */
    if (instance != INTERFACE_VENDOR_HID) {
        return 0u;
    }
    bridge_log(
        "hid get inst=%u id=%u type=%u len=%u",
        (unsigned)instance,
        (unsigned)report_id,
        (unsigned)report_type,
        (unsigned)requested_length);
    if (report_id != CODEX_REPORT_ID || buffer == NULL) {
        return 0u;
    }
    uint16_t length = requested_length;
    if (length > CODEX_REPORT_PAYLOAD_BYTES) {
        length = CODEX_REPORT_PAYLOAD_BYTES;
    }
    memset(buffer, 0, length);
    return length;
}

/* Search for a substring in a Codex report payload (bytes 3..3+length). */
static bool codex_payload_contains(
    uint8_t const *report,
    char const *needle)
{
    uint8_t chunk_len = report[2];
    if (chunk_len > CODEX_REPORT_PAYLOAD_BYTES - 3u) {
        return false;
    }
    size_t needle_len = strlen(needle);
    if (chunk_len < needle_len) {
        return false;
    }
    for (size_t i = 3u; i + needle_len <= 3u + (size_t)chunk_len; ++i) {
        if (memcmp(&report[i], needle, needle_len) == 0u) {
            return true;
        }
    }
    return false;
}

/* Parse the integer value after "id": in a Codex report payload. */
static int codex_payload_parse_id(uint8_t const *report)
{
    uint8_t chunk_len = report[2];
    char const *payload = (char const *)&report[3];
    for (size_t i = 0; i + 4 < (size_t)chunk_len; ++i) {
        if (payload[i] == '"' &&
            payload[i + 1u] == 'i' &&
            payload[i + 2u] == 'd' &&
            payload[i + 3u] == '"' &&
            payload[i + 4u] == ':') {
            size_t j = i + 5u;
            while (j < (size_t)chunk_len && payload[j] == ' ') {
                ++j;
            }
            int value = 0;
            while (j < (size_t)chunk_len &&
                   payload[j] >= '0' && payload[j] <= '9') {
                value = value * 10 + (payload[j] - '0');
                ++j;
            }
            return value;
        }
    }
    return 1;
}

/* Build and queue a device.status response as Codex Input reports. */
static void send_device_status_response(int id)
{
    char json[128];
    int json_len = snprintf(
        json,
        sizeof(json),
        "{\"result\":{\"version\":\"0.4.1\",\"profile_index\":0,"
        "\"layer_index\":0,\"battery\":100,\"is_charging\":true},\"id\":%d}",
        id);
    if (json_len <= 0 || (size_t)json_len >= sizeof(json)) {
        return;
    }
    /* Append CRLF terminator. */
    json[json_len] = '\r';
    json[json_len + 1] = '\n';
    size_t total = (size_t)json_len + 2u;
    unsigned fragments = 0u;

    /* Chunk into 61-byte Codex reports. */
    size_t offset = 0u;
    while (offset < total) {
        size_t chunk = total - offset;
        if (chunk > 61u) {
            chunk = 61u;
        }
        uint8_t report[CODEX_REPORT_WIRE_BYTES] = {0};
        report[0] = CODEX_REPORT_ID;
        report[1] = 0x02u; /* OPCODE_DATA */
        report[2] = (uint8_t)chunk;
        memcpy(&report[3], &json[offset], chunk);
        if (!report_queue_push(&input_reports, report)) {
            bridge_log("status resp queue full");
            return;
        }
        ++fragments;
        offset += chunk;
    }
    bridge_log(
        "device.status resp queued id=%d fragments=%u bytes=%u",
        id,
        fragments,
        (unsigned)total);
}

void tud_hid_set_report_cb(
    uint8_t instance,
    uint8_t report_id,
    hid_report_type_t report_type,
    uint8_t const *buffer,
    uint16_t buffer_size)
{
    /* Only handle the vendor HID instance (instance 0). */
    if (instance != INTERFACE_VENDOR_HID) {
        bridge_log(
            "hid set ignore inst=%u id=%u type=%u len=%u",
            (unsigned)instance,
            (unsigned)report_id,
            (unsigned)report_type,
            (unsigned)buffer_size);
        return;
    }
    if (buffer == NULL || buffer_size == 0u) {
        return;
    }

    uint8_t normalized[CODEX_REPORT_WIRE_BYTES] = {0};
    if (report_id == CODEX_REPORT_ID &&
        buffer_size == CODEX_REPORT_PAYLOAD_BYTES) {
        normalized[0] = CODEX_REPORT_ID;
        memcpy(&normalized[1], buffer, CODEX_REPORT_PAYLOAD_BYTES);
    } else if (buffer_size == CODEX_REPORT_WIRE_BYTES &&
               buffer[0] == CODEX_REPORT_ID) {
        memcpy(normalized, buffer, CODEX_REPORT_WIRE_BYTES);
    } else if (report_type != HID_REPORT_TYPE_OUTPUT &&
               report_type != HID_REPORT_TYPE_FEATURE) {
        bridge_log(
            "hid set skip inst=%u id=%u type=%u len=%u b0=%u",
            (unsigned)instance,
            (unsigned)report_id,
            (unsigned)report_type,
            (unsigned)buffer_size,
            (unsigned)buffer[0]);
        return;
    } else {
        bridge_log(
            "hid set drop inst=%u id=%u type=%u len=%u b0=%u",
            (unsigned)instance,
            (unsigned)report_id,
            (unsigned)report_type,
            (unsigned)buffer_size,
            (unsigned)buffer[0]);
        return;
    }

    bridge_log(
        "hid out inst=%u id=%u type=%u wire=%u chan=%u chunk=%u head=%02X%02X%02X%02X",
        (unsigned)instance,
        (unsigned)report_id,
        (unsigned)report_type,
        (unsigned)buffer_size,
        (unsigned)normalized[1],
        (unsigned)normalized[2],
        (unsigned)normalized[3],
        (unsigned)normalized[4],
        (unsigned)normalized[5],
        (unsigned)normalized[6]);

    /* Auto-respond to device.status directly via HID Input. */
    if (codex_payload_contains(normalized, "device.status")) {
        int id = codex_payload_parse_id(normalized);
        bridge_log(
            "device.status req id=%d chunk=%u",
            id,
            (unsigned)normalized[2]);
        send_device_status_response(id);
    }

    /* Also forward to bridge for logging. */
    if (!bridge_tx_push(
            BRIDGE_FRAME_CODEX_OUTPUT_REPORT,
            normalized,
            CODEX_REPORT_WIRE_BYTES)) {
        bridge_log("codex out queue full");
    }
}

void tud_hid_report_complete_cb(
    uint8_t instance,
    uint8_t const *report,
    uint16_t len)
{
    (void)report;
    if (instance != INTERFACE_VENDOR_HID) {
        return;
    }
    bridge_log(
        "hid in complete inst=%u len=%u",
        (unsigned)instance,
        (unsigned)len);
}

void tud_mount_cb(void)
{
    bridge_log("usb mounted");
}

void tud_umount_cb(void)
{
    bridge_log("usb unmounted");
}

void tud_suspend_cb(bool remote_wakeup_enabled)
{
    bridge_log("usb suspended wake=%u", (unsigned)remote_wakeup_enabled);
}

void tud_resume_cb(void)
{
    bridge_log("usb resumed");
}

void tud_hid_set_protocol_cb(uint8_t instance, uint8_t protocol)
{
    (void)instance;
    bridge_log("hid protocol=%u", (unsigned)protocol);
}

bool tud_hid_set_idle_cb(uint8_t instance, uint8_t idle_rate)
{
    (void)instance;
    bridge_log("hid idle=%u", (unsigned)idle_rate);
    return true;
}
