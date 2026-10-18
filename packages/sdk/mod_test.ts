import { assertEquals } from "@std/assert";
import { PROTOCOL_VERSION } from "./mod.ts";

Deno.test("re-exports the protocol version", () => {
  assertEquals(PROTOCOL_VERSION, "0.1.0");
});
