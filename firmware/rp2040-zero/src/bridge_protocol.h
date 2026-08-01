#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define BRIDGE_MAGIC_0 0x43u
#define BRIDGE_MAGIC_1 0x4Du
#define BRIDGE_VERSION 0x01u
#define BRIDGE_MAX_PAYLOAD 64u
#define BRIDGE_HEADER_BYTES 8u
#define BRIDGE_CRC_BYTES 2u
#define BRIDGE_MAX_FRAME_BYTES \
    (BRIDGE_HEADER_BYTES + BRIDGE_MAX_PAYLOAD + BRIDGE_CRC_BYTES)

typedef enum {
    BRIDGE_FRAME_CODEX_INPUT_REPORT = 0x01,
    BRIDGE_FRAME_CODEX_OUTPUT_REPORT = 0x02,
    BRIDGE_FRAME_PING = 0x03,
    BRIDGE_FRAME_STATUS = 0x04,
    BRIDGE_FRAME_LOG = 0x05,
} bridge_frame_type_t;

typedef struct {
    uint8_t type;
    uint16_t sequence;
    uint16_t length;
    uint8_t payload[BRIDGE_MAX_PAYLOAD];
} bridge_frame_t;

typedef struct {
    uint8_t bytes[BRIDGE_MAX_FRAME_BYTES * 2u];
    size_t length;
} bridge_parser_t;

typedef void (*bridge_frame_callback_t)(
    bridge_frame_t const *frame,
    void *context);

uint16_t bridge_crc16_ccitt(uint8_t const *bytes, size_t length);

size_t bridge_frame_encode(
    bridge_frame_t const *frame,
    uint8_t *destination,
    size_t capacity);

void bridge_parser_init(bridge_parser_t *parser);

void bridge_parser_feed(
    bridge_parser_t *parser,
    uint8_t const *bytes,
    size_t length,
    bridge_frame_callback_t callback,
    void *context);
