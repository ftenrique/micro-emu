import {
  MAX_CHUNK_BYTES,
  MESSAGE_TERMINATOR,
  OPCODE_DATA,
  REPORT_BYTES_BLE,
  REPORT_BYTES_USB,
  REPORT_ID,
  Transport,
} from "./constants.js";
import { ProtocolError } from "./errors.js";

const encoder = new TextEncoder();
const fatalDecoder = new TextDecoder("utf-8", { fatal: true });

function asBytes(raw) {
  if (raw instanceof Uint8Array) {
    return raw;
  }
  if (raw instanceof ArrayBuffer) {
    return new Uint8Array(raw);
  }
  if (Array.isArray(raw)) {
    return Uint8Array.from(raw);
  }
  throw new ProtocolError(
    "INVALID_REPORT_TYPE",
    "A report must be a Uint8Array, ArrayBuffer, or byte array.",
  );
}

function serializePayload(payload) {
  if (typeof payload === "string") {
    return payload;
  }
  if (payload && typeof payload === "object" && !Array.isArray(payload)) {
    return JSON.stringify(payload);
  }
  throw new ProtocolError(
    "INVALID_PAYLOAD",
    "A message payload must be a JSON string or object.",
  );
}

function joinWithTerminator(bytes) {
  const terminated = new Uint8Array(bytes.length + MESSAGE_TERMINATOR.length);
  terminated.set(bytes);
  terminated.set(MESSAGE_TERMINATOR, bytes.length);
  return terminated;
}

export function frameMessage(payload, options = {}) {
  const transport = options.transport ?? Transport.USB;
  if (!Object.values(Transport).includes(transport)) {
    throw new ProtocolError(
      "INVALID_TRANSPORT",
      `Unsupported transport: ${String(transport)}`,
    );
  }

  const maxMessageBytes = options.maxMessageBytes ?? 64 * 1024;
  const message = joinWithTerminator(encoder.encode(serializePayload(payload)));
  if (message.length > maxMessageBytes) {
    throw new ProtocolError(
      "MESSAGE_TOO_LARGE",
      `Message is ${message.length} bytes; limit is ${maxMessageBytes}.`,
    );
  }

  const ble = transport === Transport.BLE;
  const reportBytes = ble ? REPORT_BYTES_BLE : REPORT_BYTES_USB;
  const headerOffset = ble ? 1 : 0;
  const reports = [];

  for (let offset = 0; offset < message.length; offset += MAX_CHUNK_BYTES) {
    const chunk = message.subarray(offset, offset + MAX_CHUNK_BYTES);
    const report = new Uint8Array(reportBytes);
    if (ble) {
      report[0] = REPORT_ID;
    }
    report[headerOffset] = OPCODE_DATA;
    report[headerOffset + 1] = chunk.length;
    report.set(chunk, headerOffset + 2);
    reports.push(report);
  }
  return reports;
}

export function frameJson(message, options = {}) {
  if (!message || typeof message !== "object" || Array.isArray(message)) {
    throw new ProtocolError(
      "INVALID_MESSAGE",
      "A JSON-RPC message must be an object.",
    );
  }
  return frameMessage(JSON.stringify(message), options);
}

export class FrameDecoder {
  #buffer = new Uint8Array();
  #maxBufferedBytes;

  constructor(options = {}) {
    this.#maxBufferedBytes = options.maxBufferedBytes ?? 64 * 1024;
  }

  get bufferedBytes() {
    return this.#buffer.length;
  }

  reset() {
    this.#buffer = new Uint8Array();
  }

  feed(raw) {
    const messages = [];
    const errors = [];
    let report;

    try {
      report = asBytes(raw);
    } catch (error) {
      errors.push(asProtocolError(error));
      return { messages, errors };
    }

    let headerOffset;
    if (
      report.length === REPORT_BYTES_BLE &&
      report[0] === REPORT_ID &&
      report[1] === OPCODE_DATA
    ) {
      headerOffset = 1;
    } else if (
      report.length === REPORT_BYTES_USB &&
      report[0] === OPCODE_DATA
    ) {
      headerOffset = 0;
    } else if (
      report.length !== REPORT_BYTES_USB &&
      report.length !== REPORT_BYTES_BLE
    ) {
      errors.push(
        new ProtocolError(
          "INVALID_REPORT_LENGTH",
          `Report length ${report.length}; expected 63 bytes or 64 with Report ID prefix.`,
          { actual: report.length },
        ),
      );
      return { messages, errors };
    } else {
      errors.push(
        new ProtocolError(
          "INVALID_REPORT_HEADER",
          "Report does not contain the expected Report ID/opcode header.",
          { firstBytes: Array.from(report.subarray(0, 3)) },
        ),
      );
      return { messages, errors };
    }

    const declaredLength = report[headerOffset + 1];
    if (declaredLength === 0 || declaredLength > MAX_CHUNK_BYTES) {
      errors.push(
        new ProtocolError(
          "INVALID_CHUNK_LENGTH",
          `Chunk length ${declaredLength}; expected 1-${MAX_CHUNK_BYTES}.`,
          { declaredLength },
        ),
      );
      return { messages, errors };
    }

    const body = report.subarray(
      headerOffset + 2,
      headerOffset + 2 + declaredLength,
    );
    const combined = new Uint8Array(this.#buffer.length + body.length);
    combined.set(this.#buffer);
    combined.set(body, this.#buffer.length);

    if (combined.length > this.#maxBufferedBytes) {
      this.reset();
      errors.push(
        new ProtocolError(
          "BUFFER_LIMIT_EXCEEDED",
          `Buffered message exceeded ${this.#maxBufferedBytes} bytes and was discarded.`,
        ),
      );
      return { messages, errors };
    }
    this.#buffer = combined;

    while (true) {
      const end = findCrlf(this.#buffer);
      if (end < 0) {
        break;
      }
      const line = this.#buffer.subarray(0, end);
      this.#buffer = this.#buffer.slice(end + 2);
      if (line.length === 0) {
        continue;
      }

      let text;
      try {
        text = fatalDecoder.decode(line);
      } catch {
        errors.push(
          new ProtocolError(
            "INVALID_UTF8",
            "A complete message contained invalid UTF-8 and was discarded.",
          ),
        );
        continue;
      }

      try {
        const parsed = JSON.parse(text);
        if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
          errors.push(
            new ProtocolError(
              "INVALID_JSON_MESSAGE",
              "A protocol message must be a JSON object.",
            ),
          );
          continue;
        }
        messages.push(parsed);
      } catch {
        errors.push(
          new ProtocolError(
            "INVALID_JSON",
            "A complete CRLF-delimited message was not valid JSON.",
          ),
        );
      }
    }

    return { messages, errors };
  }

  finish() {
    if (this.#buffer.length === 0) {
      return { messages: [], errors: [] };
    }
    const remainingBytes = this.#buffer.length;
    this.reset();
    return {
      messages: [],
      errors: [
        new ProtocolError(
          "TRUNCATED_MESSAGE",
          `Stream ended with ${remainingBytes} unterminated bytes.`,
          { remainingBytes },
        ),
      ],
    };
  }
}

export function decodeReports(reports, options = {}) {
  const decoder = new FrameDecoder(options);
  const messages = [];
  const errors = [];
  for (const report of reports) {
    const result = decoder.feed(report);
    messages.push(...result.messages);
    errors.push(...result.errors);
  }
  const final = decoder.finish();
  messages.push(...final.messages);
  errors.push(...final.errors);
  return { messages, errors };
}

function findCrlf(bytes) {
  for (let index = 0; index < bytes.length - 1; index += 1) {
    if (bytes[index] === 0x0d && bytes[index + 1] === 0x0a) {
      return index;
    }
  }
  return -1;
}

function asProtocolError(error) {
  if (error instanceof ProtocolError) {
    return error;
  }
  return new ProtocolError("DECODER_ERROR", String(error));
}
