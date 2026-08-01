#include "bridge_protocol.h"

#include <string.h>

static uint16_t read_u16_le(uint8_t const *bytes)
{
    return (uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8u);
}

static void write_u16_le(uint8_t *bytes, uint16_t value)
{
    bytes[0] = (uint8_t)(value & 0xffu);
    bytes[1] = (uint8_t)(value >> 8u);
}

uint16_t bridge_crc16_ccitt(uint8_t const *bytes, size_t length)
{
    uint16_t crc = 0xffffu;
    for (size_t index = 0; index < length; ++index) {
        crc ^= (uint16_t)bytes[index] << 8u;
        for (unsigned bit = 0; bit < 8u; ++bit) {
            crc = (crc & 0x8000u) != 0u
                ? (uint16_t)((crc << 1u) ^ 0x1021u)
                : (uint16_t)(crc << 1u);
        }
    }
    return crc;
}

size_t bridge_frame_encode(
    bridge_frame_t const *frame,
    uint8_t *destination,
    size_t capacity)
{
    if (frame == NULL || destination == NULL ||
        frame->length > BRIDGE_MAX_PAYLOAD) {
        return 0u;
    }

    size_t total = BRIDGE_HEADER_BYTES + frame->length + BRIDGE_CRC_BYTES;
    if (capacity < total) {
        return 0u;
    }

    destination[0] = BRIDGE_MAGIC_0;
    destination[1] = BRIDGE_MAGIC_1;
    destination[2] = BRIDGE_VERSION;
    destination[3] = frame->type;
    write_u16_le(&destination[4], frame->sequence);
    write_u16_le(&destination[6], frame->length);
    if (frame->length > 0u) {
        memcpy(&destination[8], frame->payload, frame->length);
    }
    uint16_t crc = bridge_crc16_ccitt(destination, total - BRIDGE_CRC_BYTES);
    write_u16_le(&destination[total - BRIDGE_CRC_BYTES], crc);
    return total;
}

void bridge_parser_init(bridge_parser_t *parser)
{
    if (parser != NULL) {
        parser->length = 0u;
    }
}

static void discard_prefix(bridge_parser_t *parser, size_t count)
{
    if (count >= parser->length) {
        parser->length = 0u;
        return;
    }
    memmove(parser->bytes, &parser->bytes[count], parser->length - count);
    parser->length -= count;
}

static void parse_available(
    bridge_parser_t *parser,
    bridge_frame_callback_t callback,
    void *context)
{
    while (parser->length >= 2u) {
        size_t magic = 0u;
        while (magic + 1u < parser->length &&
               (parser->bytes[magic] != BRIDGE_MAGIC_0 ||
                parser->bytes[magic + 1u] != BRIDGE_MAGIC_1)) {
            ++magic;
        }
        if (magic > 0u) {
            discard_prefix(parser, magic);
        }
        if (parser->length < BRIDGE_HEADER_BYTES) {
            return;
        }
        if (parser->bytes[2] != BRIDGE_VERSION) {
            discard_prefix(parser, 2u);
            continue;
        }

        uint16_t payload_length = read_u16_le(&parser->bytes[6]);
        if (payload_length > BRIDGE_MAX_PAYLOAD) {
            discard_prefix(parser, 2u);
            continue;
        }
        size_t total =
            BRIDGE_HEADER_BYTES + payload_length + BRIDGE_CRC_BYTES;
        if (parser->length < total) {
            return;
        }

        uint16_t expected = read_u16_le(
            &parser->bytes[total - BRIDGE_CRC_BYTES]);
        uint16_t actual = bridge_crc16_ccitt(
            parser->bytes,
            total - BRIDGE_CRC_BYTES);
        if (actual != expected) {
            discard_prefix(parser, 1u);
            continue;
        }

        bridge_frame_t frame = {
            .type = parser->bytes[3],
            .sequence = read_u16_le(&parser->bytes[4]),
            .length = payload_length,
        };
        if (payload_length > 0u) {
            memcpy(frame.payload, &parser->bytes[8], payload_length);
        }
        if (callback != NULL) {
            callback(&frame, context);
        }
        discard_prefix(parser, total);
    }
}

void bridge_parser_feed(
    bridge_parser_t *parser,
    uint8_t const *bytes,
    size_t length,
    bridge_frame_callback_t callback,
    void *context)
{
    if (parser == NULL || (bytes == NULL && length > 0u)) {
        return;
    }
    for (size_t offset = 0u; offset < length;) {
        size_t available = sizeof(parser->bytes) - parser->length;
        if (available == 0u) {
            discard_prefix(parser, 1u);
            available = sizeof(parser->bytes) - parser->length;
        }
        size_t chunk = length - offset;
        if (chunk > available) {
            chunk = available;
        }
        memcpy(&parser->bytes[parser->length], &bytes[offset], chunk);
        parser->length += chunk;
        offset += chunk;
        parse_available(parser, callback, context);
    }
}
