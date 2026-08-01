#include <stdio.h>
#include <string.h>

#include "bridge_protocol.h"

static unsigned failures;
static unsigned callbacks;
static bridge_frame_t last_frame;

static void expect(int condition, char const *message)
{
    if (!condition) {
        fprintf(stderr, "FAIL: %s\n", message);
        ++failures;
    }
}

static void capture_frame(bridge_frame_t const *frame, void *context)
{
    (void)context;
    ++callbacks;
    last_frame = *frame;
}

static void test_golden_ping(void)
{
    static uint8_t const expected[] = {
        0x43, 0x4d, 0x01, 0x03, 0x01,
        0x00, 0x00, 0x00, 0xbb, 0xe5,
    };
    bridge_frame_t frame = {
        .type = BRIDGE_FRAME_PING,
        .sequence = 1u,
        .length = 0u,
    };
    uint8_t encoded[BRIDGE_MAX_FRAME_BYTES] = {0};
    size_t length = bridge_frame_encode(&frame, encoded, sizeof(encoded));
    expect(length == sizeof(expected), "ping frame length");
    expect(
        memcmp(encoded, expected, sizeof(expected)) == 0,
        "C encoder matches the Rust golden ping vector");
}

static void test_fragmentation_and_noise(void)
{
    bridge_frame_t frame = {
        .type = BRIDGE_FRAME_STATUS,
        .sequence = 0x1234u,
        .length = 5u,
        .payload = {'r', 'e', 'a', 'd', 'y'},
    };
    uint8_t encoded[BRIDGE_MAX_FRAME_BYTES] = {0};
    size_t length = bridge_frame_encode(&frame, encoded, sizeof(encoded));
    uint8_t noise[] = {0xffu, 0x00u, BRIDGE_MAGIC_0};
    bridge_parser_t parser;
    bridge_parser_init(&parser);
    callbacks = 0u;

    bridge_parser_feed(&parser, noise, sizeof(noise), capture_frame, NULL);
    bridge_parser_feed(&parser, encoded, 3u, capture_frame, NULL);
    bridge_parser_feed(
        &parser,
        &encoded[3],
        length - 3u,
        capture_frame,
        NULL);

    expect(callbacks == 1u, "fragmented frame produces one callback");
    expect(last_frame.type == BRIDGE_FRAME_STATUS, "status type survives");
    expect(last_frame.sequence == 0x1234u, "sequence survives");
    expect(last_frame.length == 5u, "payload length survives");
    expect(
        memcmp(last_frame.payload, "ready", 5u) == 0,
        "payload survives");
}

static void test_crc_recovery(void)
{
    bridge_frame_t bad = {
        .type = BRIDGE_FRAME_PING,
        .sequence = 7u,
        .length = 0u,
    };
    bridge_frame_t good = {
        .type = BRIDGE_FRAME_STATUS,
        .sequence = 8u,
        .length = 2u,
        .payload = {'o', 'k'},
    };
    uint8_t bytes[BRIDGE_MAX_FRAME_BYTES * 2u] = {0};
    size_t bad_length = bridge_frame_encode(&bad, bytes, sizeof(bytes));
    bytes[3] ^= 1u;
    size_t good_length = bridge_frame_encode(
        &good,
        &bytes[bad_length],
        sizeof(bytes) - bad_length);

    bridge_parser_t parser;
    bridge_parser_init(&parser);
    callbacks = 0u;
    bridge_parser_feed(
        &parser,
        bytes,
        bad_length + good_length,
        capture_frame,
        NULL);

    expect(callbacks == 1u, "parser ignores corrupt frame and recovers");
    expect(last_frame.sequence == 8u, "recovered frame is the valid one");
}

int main(void)
{
    test_golden_ping();
    test_fragmentation_and_noise();
    test_crc_recovery();
    if (failures != 0u) {
        return 1;
    }
    puts("{\"firmwareHostTests\":3,\"passed\":true}");
    return 0;
}
