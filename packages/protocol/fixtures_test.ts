// Golden vectors shared with the Rust side (ADR 0016).
//
// `protocol/fixtures/encoded/ts.json` holds the frames this package produces;
// regenerate it with `COVALENCE_UPDATE_FIXTURES=1 deno task test`. The Rust
// tests decode it, and this test decodes the frames Rust produced.

import { assert, assertEquals, assertThrows } from "@std/assert";
import { decodeHex, encodeHex } from "@std/encoding/hex";
import { Ajv2020 } from "ajv/dist/2020.js";
import schema from "../../protocol/json-schema/covalence-protocol.json" with {
  type: "json",
};
import messages from "../../protocol/fixtures/messages.json" with {
  type: "json",
};
import invalid from "../../protocol/fixtures/invalid.json" with {
  type: "json",
};
import rustFrames from "../../protocol/fixtures/encoded/rust.json" with {
  type: "json",
};
import {
  checkMessage,
  encodeFrame,
  type Format,
  FrameDecoder,
} from "./framing.ts";
import type { Message } from "./types.gen.ts";

interface Fixture {
  name: string;
  params?: string;
  result?: string;
  message: unknown;
}

const encodedDir = new URL("../../protocol/fixtures/encoded/", import.meta.url);

const ajv = new Ajv2020({ strict: false });
ajv.addSchema(schema);

function validate(def: string, value: unknown): string | undefined {
  const check = ajv.getSchema(`${def}.json`);
  assert(check, `no schema definition ${def}`);
  return check(value) ? undefined : ajv.errorsText(check.errors);
}

/** Every reason a fixture would be rejected: envelope, schema, params, result. */
function problems(fixture: Fixture): string[] {
  const out: string[] = [];
  try {
    checkMessage(fixture.message);
  } catch (e) {
    out.push((e as Error).message);
  }
  const envelope = validate("Message", fixture.message);
  if (envelope) out.push(`envelope: ${envelope}`);
  const msg = fixture.message as Record<string, unknown>;
  if (fixture.params) {
    const params = validate(fixture.params, msg.p);
    if (params) out.push(`params: ${params}`);
  }
  if (fixture.result && msg.ok === true) {
    const result = validate(fixture.result, msg.r);
    if (result) out.push(`result: ${result}`);
  }
  return out;
}

function roundtrip(message: Message, format: Format): Message | undefined {
  const decoder = new FrameDecoder(format);
  decoder.push(encodeFrame(message, format));
  return decoder.next();
}

Deno.test("valid messages pass the schema and round-trip", () => {
  for (const fixture of messages as Fixture[]) {
    assertEquals(problems(fixture), [], fixture.name);
    const message = fixture.message as Message;
    for (const format of ["msgpack", "json"] as const) {
      assertEquals(
        roundtrip(message, format),
        message,
        `${fixture.name}: ${format}`,
      );
    }
  }
});

Deno.test("invalid messages are rejected", () => {
  for (const fixture of invalid as Fixture[]) {
    assert(problems(fixture).length > 0, `${fixture.name}: accepted`);
  }
});

Deno.test("typescript frames are current", async () => {
  const frames = Object.fromEntries(
    (messages as Fixture[])
      .map((
        f,
      ) => [f.name, encodeHex(encodeFrame(f.message as Message, "msgpack"))])
      .sort(([a], [b]) => a < b ? -1 : 1),
  );
  const expected = JSON.stringify(frames, null, 2) + "\n";
  const path = new URL("ts.json", encodedDir);

  if (Deno.env.get("COVALENCE_UPDATE_FIXTURES")) {
    await Deno.writeTextFile(path, expected);
  }
  const actual = await Deno.readTextFile(path).catch(() => "");
  assert(
    actual === expected,
    `${path.pathname} is stale; rerun with COVALENCE_UPDATE_FIXTURES=1`,
  );
});

Deno.test("rust frames decode", () => {
  const frames = rustFrames as Record<string, string>;
  assertEquals(
    Object.keys(frames).length,
    messages.length,
    "rust.json is stale",
  );
  for (const fixture of messages as Fixture[]) {
    const frame = frames[fixture.name];
    assert(frame, `${fixture.name}: no Rust frame`);
    const decoder = new FrameDecoder("msgpack");
    decoder.push(decodeHex(frame));
    const message = decoder.next();
    assertEquals(message, fixture.message, fixture.name);
    assertEquals(problems({ ...fixture, message }), [], fixture.name);
  }
});

Deno.test("binary and 64-bit integers survive MessagePack; bad frames are skipped", () => {
  const message: Message = {
    t: "ntf",
    m: "log",
    p: { blob: new Uint8Array([0, 1, 255]) },
  };
  assertEquals(roundtrip(message, "msgpack"), message);

  // Integers that needed 64 bits on the wire come back as numbers when safe.
  const big: Message = {
    t: "ntf",
    m: "log",
    p: { ts: 1760781600000, huge: 2n ** 63n },
  };
  assertEquals(roundtrip(big, "msgpack"), big);

  const decoder = new FrameDecoder("msgpack");
  decoder.push(new Uint8Array([0, 0, 0, 1, 0xc1])); // 0xc1 is never valid MessagePack
  decoder.push(encodeFrame({ t: "ntf", m: "log", p: null }, "msgpack"));
  assertThrows(() => decoder.next());
  assertEquals(decoder.next(), { t: "ntf", m: "log", p: null });
});
