/**
 * Framing. Spec §2.
 *
 * - MessagePack (default): a `u32` big-endian length, then the body.
 * - JSON (`COVALENCE_PROTOCOL_DEBUG=json`): one message per line.
 *
 * @module
 */

import { decode as decodeMsgPack, encode as encodeMsgPack } from "@std/msgpack";
import type { Message } from "./types.gen.ts";

/** Largest frame body accepted. Larger payloads must be chunked. */
export const MAX_FRAME_LEN = 16 * 1024 * 1024;

export type Format = "msgpack" | "json";

/** JSON if `COVALENCE_PROTOCOL_DEBUG=json`, otherwise MessagePack. */
export function formatFromEnv(): Format {
  try {
    return Deno.env.get("COVALENCE_PROTOCOL_DEBUG") === "json"
      ? "json"
      : "msgpack";
  } catch {
    return "msgpack"; // no env permission
  }
}

export class FrameError extends Error {
  /**
   * @param fatal The stream can't be realigned; drop the link. When false,
   * the bad frame was consumed and decoding can continue.
   */
  constructor(
    message: string,
    readonly fatal: boolean,
    options?: ErrorOptions,
  ) {
    super(message, options);
    this.name = "FrameError";
  }
}

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

/** Encodes one message as a complete frame. */
export function encodeFrame(message: Message, format: Format): Uint8Array {
  if (format === "json") {
    const line = textEncoder.encode(JSON.stringify(message, jsonReplacer));
    checkLen(line.length);
    const frame = new Uint8Array(line.length + 1);
    frame.set(line);
    frame[line.length] = 0x0a;
    return frame;
  }
  // deno-lint-ignore no-explicit-any
  const body = encodeMsgPack(prune(message) as any);
  checkLen(body.length);
  const frame = new Uint8Array(4 + body.length);
  new DataView(frame.buffer).setUint32(0, body.length);
  frame.set(body, 4);
  return frame;
}

function checkLen(len: number) {
  if (len > MAX_FRAME_LEN) {
    throw new FrameError(
      `frame of ${len} bytes exceeds the ${MAX_FRAME_LEN}-byte limit`,
      true,
    );
  }
}

// Binary goes out as an array of byte values, as serde does on the Rust side.
function jsonReplacer(_key: string, value: unknown) {
  return value instanceof Uint8Array ? Array.from(value) : value;
}

// MessagePack has no `undefined`. Drop it from objects and turn it into null
// in arrays, as JSON does.
function prune(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map((v) => v === undefined ? null : prune(v));
  }
  if (
    value === null || typeof value !== "object" || value instanceof Uint8Array
  ) return value;
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(value)) {
    if (v !== undefined) out[k] = prune(v);
  }
  return out;
}

// @std/msgpack decodes every 64-bit integer as a bigint. Peers send 64-bit
// integers whenever a value needs more than 32 bits (timestamps, for example),
// so turn those within the safe range back into numbers (state spec §1.2).
function safeInts(value: unknown): unknown {
  if (typeof value === "bigint") {
    return value >= BigInt(Number.MIN_SAFE_INTEGER) &&
        value <= BigInt(Number.MAX_SAFE_INTEGER)
      ? Number(value)
      : value;
  }
  if (Array.isArray(value)) return value.map(safeInts);
  if (
    value === null || typeof value !== "object" || value instanceof Uint8Array
  ) return value;
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(value)) out[k] = safeInts(v);
  return out;
}

/** Splits a byte stream into messages. */
export class FrameDecoder {
  #buf = new Uint8Array(0);

  constructor(readonly format: Format) {}

  /** Appends bytes read from the link. */
  push(data: Uint8Array) {
    const buf = new Uint8Array(this.#buf.length + data.length);
    buf.set(this.#buf);
    buf.set(data, this.#buf.length);
    this.#buf = buf;
  }

  /** Returns the next complete message, or `undefined` if more bytes are needed. */
  next(): Message | undefined {
    return this.format === "json" ? this.#nextJson() : this.#nextMsgPack();
  }

  #nextMsgPack(): Message | undefined {
    if (this.#buf.length < 4) return undefined;
    const len = new DataView(this.#buf.buffer, this.#buf.byteOffset).getUint32(
      0,
    );
    if (len > MAX_FRAME_LEN) {
      throw new FrameError(
        `frame of ${len} bytes exceeds the ${MAX_FRAME_LEN}-byte limit`,
        true,
      );
    }
    if (this.#buf.length < 4 + len) return undefined;
    const body = this.#buf.subarray(4, 4 + len);
    this.#buf = this.#buf.slice(4 + len);
    let value: unknown;
    try {
      value = decodeMsgPack(body);
    } catch (cause) {
      throw new FrameError("invalid MessagePack frame", false, { cause });
    }
    return checkMessage(safeInts(value));
  }

  #nextJson(): Message | undefined {
    for (;;) {
      const end = this.#buf.indexOf(0x0a);
      if (end < 0) {
        if (this.#buf.length > MAX_FRAME_LEN) {
          throw new FrameError(
            `line exceeds the ${MAX_FRAME_LEN}-byte limit`,
            true,
          );
        }
        return undefined;
      }
      const line = textDecoder.decode(this.#buf.subarray(0, end)).trim();
      this.#buf = this.#buf.slice(end + 1);
      if (!line) continue;
      let value: unknown;
      try {
        value = JSON.parse(line);
      } catch (cause) {
        throw new FrameError("invalid JSON frame", false, { cause });
      }
      return checkMessage(value);
    }
  }
}

/**
 * Checks the envelope shape (spec §2.1), with the same rules as the Rust
 * decoder. Params are not checked here.
 */
export function checkMessage(value: unknown): Message {
  const fail = (why: string): never => {
    throw new FrameError(`invalid message: ${why}`, false);
  };
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail("not an object");
  }
  const msg = value as Record<string, unknown>;
  const isId = (id: unknown) =>
    typeof id === "number" && Number.isInteger(id) && id >= 0 &&
    id <= 0xffffffff;
  const isOptString = (s: unknown) => s === undefined || typeof s === "string";

  switch (msg.t) {
    case "req":
      if (!isId(msg.id)) fail("`id` must be a u32");
      if (typeof msg.m !== "string") fail("`m` must be a string");
      if (!isOptString(msg.conn)) fail("`conn` must be a string");
      if (!("p" in msg)) fail("missing `p`");
      break;
    case "ntf":
      if (typeof msg.m !== "string") fail("`m` must be a string");
      if (!isOptString(msg.conn)) fail("`conn` must be a string");
      if (!("p" in msg)) fail("missing `p`");
      break;
    case "res":
      if (!isId(msg.id)) fail("`id` must be a u32");
      if (msg.ok === true) {
        if (!("r" in msg) || "e" in msg) {
          fail("a response with `ok: true` needs `r` and no `e`");
        }
      } else if (msg.ok === false) {
        if (!("e" in msg) || "r" in msg) {
          fail("a response with `ok: false` needs `e` and no `r`");
        }
        const e = msg.e as Record<string, unknown> | null;
        if (
          e === null || typeof e !== "object" || typeof e.code !== "string" ||
          typeof e.message !== "string"
        ) {
          fail("`e` must have a string `code` and `message`");
        }
      } else {
        fail("`ok` must be a boolean");
      }
      break;
    default:
      fail("`t` must be one of req, res, ntf");
  }
  return msg as unknown as Message;
}
