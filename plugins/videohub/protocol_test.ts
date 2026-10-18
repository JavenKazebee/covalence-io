import { assertEquals } from "@std/assert";
import {
  BlockDecoder,
  formatBlock,
  parseFields,
  parseLabels,
  parseRoutes,
} from "./protocol.ts";

Deno.test("decodes blocks split across chunks, with CRLF", () => {
  const d = new BlockDecoder();
  assertEquals(d.push("VIDEO OUTPUT ROUTING:\r\n0 5\n1 "), []);
  assertEquals(d.push("3\n\nACK\n\n\n"), [
    { header: "VIDEO OUTPUT ROUTING", lines: ["0 5", "1 3"] },
    { header: "ACK", lines: [] },
  ]);
});

Deno.test("formats blocks", () => {
  assertEquals(
    formatBlock("VIDEO OUTPUT ROUTING", ["3 5"]),
    "VIDEO OUTPUT ROUTING:\n3 5\n\n",
  );
  assertEquals(formatBlock("ACK"), "ACK\n\n");
  assertEquals(formatBlock("END PRELUDE"), "END PRELUDE:\n\n");
});

Deno.test("parses fields, labels and routes", () => {
  assertEquals(
    parseFields(["Model name: Smart Videohub", "Video inputs: 12"]),
    new Map([["Model name", "Smart Videohub"], ["Video inputs", "12"]]),
  );
  assertEquals(
    parseLabels(["0 Cam 1", "1 ", "x"]),
    new Map([[0, "Cam 1"], [1, ""]]),
  );
  assertEquals(parseRoutes(["0 5", "bad", "2 1"]), new Map([[0, 5], [2, 1]]));
});
