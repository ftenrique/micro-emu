import { Effects, Methods } from "./constants.js";
import { ProtocolError } from "./errors.js";

const AGENT_KEY_PATTERN = /^AG0[0-5]$/;
const ACTION_KEY_PATTERN = /^ACT(?:0[6-9]|1[0-2])$/;
const ENCODER_KEYS = new Set(["ENC_CLK", "ENC_CW", "ENC_CC"]);

function assertRecord(value, field) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new ProtocolError("INVALID_FIELD", `${field} must be an object.`);
  }
}

function assertMethod(method) {
  if (typeof method !== "string" || method.length === 0) {
    throw new ProtocolError(
      "INVALID_METHOD",
      "Method must be a non-empty string.",
    );
  }
}

function assertId(id) {
  if (
    id !== null &&
    typeof id !== "string" &&
    !(typeof id === "number" && Number.isFinite(id))
  ) {
    throw new ProtocolError(
      "INVALID_ID",
      "Request id must be a finite number, string, or null.",
    );
  }
}

export function createRequest(method, params = undefined, id = 1, options = {}) {
  assertMethod(method);
  assertId(id);
  if (options.standardJsonRpc === true) {
    return {
      jsonrpc: "2.0",
      method,
      ...(params === undefined ? {} : { params }),
      id,
    };
  }
  return { m: method, ...(params === undefined ? {} : { p: params }), id };
}

export function createNotification(method, params, options = {}) {
  assertMethod(method);
  if (options.standardJsonRpc === true) {
    return {
      jsonrpc: "2.0",
      method,
      ...(params === undefined ? {} : { params }),
    };
  }
  return { m: method, ...(params === undefined ? {} : { p: params }) };
}

export function parseRpcMessage(message) {
  assertRecord(message, "message");
  const method = message.m ?? message.method;
  if (method !== undefined) {
    assertMethod(method);
  }
  const hasCompactParams = Object.hasOwn(message, "p");
  const hasStandardParams = Object.hasOwn(message, "params");
  return {
    style: Object.hasOwn(message, "m") ? "compact" : "standard",
    method,
    params: hasCompactParams
      ? message.p
      : hasStandardParams
        ? message.params
        : undefined,
    id: Object.hasOwn(message, "id") ? message.id : undefined,
    result: message.result,
    error: message.error,
    raw: message,
  };
}

export function deviceStatusRequest(id = 1) {
  return createRequest(Methods.DEVICE_STATUS, undefined, id);
}

export function deviceStatusResponse(id, status) {
  assertId(id);
  assertRecord(status, "status");
  const normalized = {
    version: requireString(status.version, "version"),
    profile_index: requireInteger(status.profile_index, "profile_index", 0),
    layer_index: requireInteger(status.layer_index, "layer_index", 0),
    battery: requireNumber(status.battery, "battery", 0, 100),
    is_charging: requireBoolean(status.is_charging, "is_charging"),
  };
  return { result: normalized, id };
}

export function keyEvent(key, act, agentIndex = undefined) {
  if (
    typeof key !== "string" ||
    !(
      AGENT_KEY_PATTERN.test(key) ||
      ACTION_KEY_PATTERN.test(key) ||
      ENCODER_KEYS.has(key)
    )
  ) {
    throw new ProtocolError("INVALID_KEY", `Unknown Codex Micro key: ${key}`);
  }
  if (!Number.isInteger(act)) {
    throw new ProtocolError("INVALID_ACTION", "act must be an integer.");
  }
  if (!key.startsWith("ENC_") && act !== 0 && act !== 1) {
    throw new ProtocolError(
      "INVALID_ACTION",
      "Physical keys use act 1 for down and 0 for up.",
    );
  }
  if (
    agentIndex !== undefined &&
    (!Number.isInteger(agentIndex) || agentIndex < 0 || agentIndex > 5)
  ) {
    throw new ProtocolError(
      "INVALID_AGENT_INDEX",
      "Agent index must be an integer from 0 to 5.",
    );
  }
  return createNotification(Methods.KEY_EVENT, {
    k: key,
    act,
    ...(agentIndex === undefined ? {} : { ag: agentIndex }),
  });
}

export function radialEvent(angle, distance) {
  return createNotification(Methods.RADIAL_EVENT, {
    a: requireNumber(angle, "angle", 0, 1),
    d: requireNumber(distance, "distance", 0, 1),
  });
}

export function lightingSide({
  color,
  brightness = 1,
  effect = Effects.SOLID,
  speed = 0,
  magic = undefined,
}) {
  const side = {
    e: requireInteger(effect, "effect", 0, 6),
    b: requireNumber(brightness, "brightness", 0, 1),
    s: requireNumber(speed, "speed", 0, 1),
    c: normalizeColor(color),
  };
  if (magic !== undefined) {
    side.m = requireNumber(magic, "magic", 0, 1);
  }
  return side;
}

export function rgbConfig({ ambient = undefined, keys = undefined }) {
  if (ambient === undefined && keys === undefined) {
    throw new ProtocolError(
      "EMPTY_RGB_CONFIG",
      "At least one of ambient or keys is required.",
    );
  }
  return createNotification(Methods.RGB_CONFIG, {
    ...(ambient === undefined ? {} : { ambient: lightingSide(ambient) }),
    ...(keys === undefined ? {} : { keys: lightingSide(keys) }),
  });
}

export function threadStatusEntry({
  id,
  color,
  brightness = 1,
  effect = Effects.SOLID,
  speed = 0,
  syncKeys = undefined,
  syncAmbient = undefined,
}) {
  const entry = {
    id: requireInteger(id, "id", 0, 5),
    c: normalizeColor(color),
    b: requireNumber(brightness, "brightness", 0, 1),
    e: requireInteger(effect, "effect", 0, 6),
    s: requireNumber(speed, "speed", 0, 1),
  };
  if (syncKeys !== undefined) {
    entry.sk = requireBoolean(syncKeys, "syncKeys") ? 1 : 0;
  }
  if (syncAmbient !== undefined) {
    entry.sa = requireBoolean(syncAmbient, "syncAmbient") ? 1 : 0;
  }
  return entry;
}

export function threadStatus(entries) {
  if (!Array.isArray(entries) || entries.length === 0) {
    throw new ProtocolError(
      "EMPTY_THREAD_STATUS",
      "At least one thread status entry is required.",
    );
  }
  const normalized = entries.map(threadStatusEntry);
  const uniqueIds = new Set(normalized.map((entry) => entry.id));
  if (uniqueIds.size !== normalized.length) {
    throw new ProtocolError(
      "DUPLICATE_THREAD_ID",
      "Thread status entries must have unique ids.",
    );
  }
  return createNotification(Methods.THREAD_STATUS, normalized);
}

export function methodNotFoundResponse(requestOrId) {
  let id = requestOrId;
  if (
    requestOrId &&
    typeof requestOrId === "object" &&
    !Array.isArray(requestOrId)
  ) {
    id = Object.hasOwn(requestOrId, "id") ? requestOrId.id : null;
  }
  assertId(id);
  return {
    error: { code: 404, message: "Method not found" },
    id,
  };
}

export function internalErrorResponse(requestOrId) {
  let id = requestOrId;
  if (
    requestOrId &&
    typeof requestOrId === "object" &&
    !Array.isArray(requestOrId)
  ) {
    id = Object.hasOwn(requestOrId, "id") ? requestOrId.id : null;
  }
  assertId(id);
  return {
    error: { code: 500, message: "Internal error" },
    id,
  };
}

export function dispatchRequest(message, handlers) {
  assertRecord(handlers, "handlers");
  const parsed = parseRpcMessage(message);
  if (!parsed.method || typeof handlers[parsed.method] !== "function") {
    return methodNotFoundResponse(message);
  }
  try {
    return handlers[parsed.method](parsed.params, parsed.id, parsed);
  } catch {
    return internalErrorResponse(message);
  }
}

export function normalizeColor(value) {
  if (Number.isInteger(value) && value >= 0 && value <= 0xffffff) {
    return value;
  }
  if (typeof value === "string") {
    let hex = value.trim().replace(/^#/, "").replace(/^0x/i, "");
    if (/^[0-9a-f]{3}$/i.test(hex)) {
      hex = [...hex].map((digit) => digit + digit).join("");
    }
    if (/^[0-9a-f]{6}$/i.test(hex)) {
      return Number.parseInt(hex, 16);
    }
  }
  if (
    Array.isArray(value) &&
    value.length === 3 &&
    value.every((part) => Number.isInteger(part) && part >= 0 && part <= 255)
  ) {
    return (value[0] << 16) | (value[1] << 8) | value[2];
  }
  throw new ProtocolError(
    "INVALID_COLOR",
    "Color must be 0x000000-0xFFFFFF, #RRGGBB, #RGB, or [r,g,b].",
  );
}

function requireString(value, field) {
  if (typeof value !== "string" || value.length === 0) {
    throw new ProtocolError("INVALID_FIELD", `${field} must be a string.`);
  }
  return value;
}

function requireBoolean(value, field) {
  if (typeof value !== "boolean") {
    throw new ProtocolError("INVALID_FIELD", `${field} must be boolean.`);
  }
  return value;
}

function requireInteger(value, field, minimum, maximum = Infinity) {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new ProtocolError(
      "INVALID_FIELD",
      `${field} must be an integer from ${minimum} to ${maximum}.`,
    );
  }
  return value;
}

function requireNumber(value, field, minimum, maximum) {
  if (
    typeof value !== "number" ||
    !Number.isFinite(value) ||
    value < minimum ||
    value > maximum
  ) {
    throw new ProtocolError(
      "INVALID_FIELD",
      `${field} must be a number from ${minimum} to ${maximum}.`,
    );
  }
  return value;
}
