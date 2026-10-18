import { assertEquals, assertThrows } from "@std/assert";
import { formatPath, parsePath, toPath } from "./paths.ts";

Deno.test("parses the text form into segments", () => {
  assertEquals(parsePath("outputs[3].source"), ["outputs", 3, "source"]);
  assertEquals(parsePath('outputs["SDI 4"].source'), [
    "outputs",
    "SDI 4",
    "source",
  ]);
  assertEquals(parsePath("a['x\\'y'][-1]"), ["a", "x'y", -1]);
  assertEquals(parsePath("[2].name"), [2, "name"]);
  assertEquals(parsePath(""), []);
});

Deno.test("rejects malformed paths", () => {
  for (
    const bad of ["a..b", "a.", "1a", "a[", "a[x]", 'a["x', "a b", "a.[1]"]
  ) {
    assertThrows(() => parsePath(bad), SyntaxError, undefined, bad);
  }
});

Deno.test("formats segments back to text", () => {
  for (
    const text of ["outputs[3].source", 'outputs["SDI 4"].source', "a.b_c"]
  ) {
    assertEquals(formatPath(parsePath(text)), text);
  }
  assertEquals(formatPath(["a", "3"]), 'a["3"]');
});

Deno.test("passes segment arrays through and checks them", () => {
  assertEquals(toPath(["a", 1]), ["a", 1]);
  assertThrows(() => toPath(["a", 1.5]), TypeError);
});
