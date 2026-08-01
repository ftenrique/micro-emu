import assert from "node:assert/strict";
import test from "node:test";

import {
  Effects,
  Methods,
  createNotification,
  createRequest,
  deviceStatusRequest,
  deviceStatusResponse,
  dispatchRequest,
  keyEvent,
  methodNotFoundResponse,
  normalizeColor,
  parseRpcMessage,
  radialEvent,
  rgbConfig,
  threadStatus,
} from "../../protocol/index.js";

test("compact and standard JSON-RPC requests are modeled", () => {
  assert.deepEqual(createRequest("device.status", undefined, 7), {
    m: "device.status",
    id: 7,
  });
  assert.deepEqual(
    createRequest("device.status", {}, "a", { standardJsonRpc: true }),
    { jsonrpc: "2.0", method: "device.status", params: {}, id: "a" },
  );
  assert.deepEqual(
    createNotification("v.oai.hid", { k: "AG00" }),
    { m: "v.oai.hid", p: { k: "AG00" } },
  );
});

test("parser normalizes compact and standard envelopes", () => {
  assert.deepEqual(
    parseRpcMessage({ m: "x", p: { a: 1 }, id: 2 }).method,
    "x",
  );
  const standard = parseRpcMessage({
    jsonrpc: "2.0",
    method: "y",
    params: [1],
  });
  assert.equal(standard.style, "standard");
  assert.deepEqual(standard.params, [1]);
});

test("device.status request and response validate their contract", () => {
  assert.deepEqual(deviceStatusRequest(1), { m: Methods.DEVICE_STATUS, id: 1 });
  assert.deepEqual(
    deviceStatusResponse(1, {
      version: "0.4.1",
      profile_index: 0,
      layer_index: 0,
      battery: 88,
      is_charging: false,
    }),
    {
      result: {
        version: "0.4.1",
        profile_index: 0,
        layer_index: 0,
        battery: 88,
        is_charging: false,
      },
      id: 1,
    },
  );
  assert.throws(
    () =>
      deviceStatusResponse(1, {
        version: "x",
        profile_index: 0,
        layer_index: 0,
        battery: 101,
        is_charging: false,
      }),
    (error) => error.code === "INVALID_FIELD",
  );
});

test("key events model agent, action, and encoder inputs", () => {
  assert.deepEqual(keyEvent("AG00", 1, 0), {
    m: Methods.KEY_EVENT,
    p: { k: "AG00", act: 1, ag: 0 },
  });
  assert.deepEqual(keyEvent("ACT12", 0), {
    m: Methods.KEY_EVENT,
    p: { k: "ACT12", act: 0 },
  });
  assert.equal(keyEvent("ENC_CW", 4).p.act, 4);
  assert.throws(
    () => keyEvent("AG06", 1),
    (error) => error.code === "INVALID_KEY",
  );
  assert.throws(
    () => keyEvent("ACT06", 2),
    (error) => error.code === "INVALID_ACTION",
  );
});

test("radial events are normalized values", () => {
  assert.deepEqual(radialEvent(0.25, 1), {
    m: Methods.RADIAL_EVENT,
    p: { a: 0.25, d: 1 },
  });
  assert.throws(() => radialEvent(-0.1, 0));
});

test("colors support integer, hex, shorthand, and RGB tuple", () => {
  assert.equal(normalizeColor(0x12abef), 0x12abef);
  assert.equal(normalizeColor("#12ABEF"), 0x12abef);
  assert.equal(normalizeColor("#0f8"), 0x00ff88);
  assert.equal(normalizeColor([1, 2, 3]), 0x010203);
  assert.throws(
    () => normalizeColor("#xyz"),
    (error) => error.code === "INVALID_COLOR",
  );
});

test("rgbcfg uses minimized vendor fields and no id", () => {
  const message = rgbConfig({
    ambient: {
      color: "#ff0000",
      effect: Effects.BREATH,
      brightness: 0.5,
      speed: 0.25,
    },
  });
  assert.deepEqual(message, {
    m: Methods.RGB_CONFIG,
    p: { ambient: { e: 4, b: 0.5, s: 0.25, c: 0xff0000 } },
  });
  assert.equal(Object.hasOwn(message, "id"), false);
  assert.throws(() => rgbConfig({}));
});

test("thstatus validates ids, uniqueness, and optional sync flags", () => {
  const message = threadStatus([
    { id: 0, color: "#ffffff", syncKeys: true },
    { id: 5, color: 0, effect: Effects.OFF, brightness: 0 },
  ]);
  assert.equal(message.m, Methods.THREAD_STATUS);
  assert.deepEqual(message.p[0], {
    id: 0,
    c: 0xffffff,
    b: 1,
    e: 1,
    s: 0,
    sk: 1,
  });
  assert.throws(
    () =>
      threadStatus([
        { id: 0, color: 0 },
        { id: 0, color: 1 },
      ]),
    (error) => error.code === "DUPLICATE_THREAD_ID",
  );
});

test("unknown methods receive a safe correlated 404", () => {
  assert.deepEqual(methodNotFoundResponse({ m: "unknown", id: 9 }), {
    error: { code: 404, message: "Method not found" },
    id: 9,
  });
  assert.deepEqual(dispatchRequest({ m: "unknown", id: 9 }, {}), {
    error: { code: 404, message: "Method not found" },
    id: 9,
  });
});

test("dispatchRequest contains handler exceptions", () => {
  const response = dispatchRequest(
    { m: "explode", id: 4 },
    {
      explode() {
        throw new Error("secret detail");
      },
    },
  );
  assert.deepEqual(response, {
    error: { code: 500, message: "Internal error" },
    id: 4,
  });
  assert.equal(JSON.stringify(response).includes("secret"), false);
});
