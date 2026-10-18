import { assertEquals } from "@std/assert";
import { PatchCoalescer, StateTree } from "./state.ts";

Deno.test("set splits objects into leaf ops with integer keys", () => {
  const t = new StateTree();
  const ops = t.set([], { outputs: { 0: { name: "PGM", source: 2 } } });
  assertEquals(ops, [
    { p: ["outputs", 0, "name"], v: "PGM" },
    { p: ["outputs", 0, "source"], v: 2 },
  ]);
  assertEquals(t.get(["outputs", 0, "source"]), 2);
  assertEquals(t.get([]), { outputs: { 0: { name: "PGM", source: 2 } } });
});

Deno.test("unchanged writes produce no ops", () => {
  const t = new StateTree();
  t.set(["a"], { b: 1, c: [1, 2] });
  assertEquals(t.patch(["a", "b"], 1), []);
  assertEquals(t.set(["a"], { b: 1, c: [1, 2] }), []);
});

Deno.test("set deletes missing keys, patch keeps them", () => {
  const t = new StateTree();
  t.set([], { a: { x: 1, y: 2 } });
  assertEquals(t.patch(["a"], { x: 3 }), [{ p: ["a", "x"], v: 3 }]);
  assertEquals(t.set(["a"], { x: 3 }), [{ p: ["a", "y"], del: true }]);
  assertEquals(t.get(["a"]), { x: 3 });
});

Deno.test("replacing a branch with a leaf deletes the branch first", () => {
  const t = new StateTree();
  t.set(["a"], { x: 1 });
  assertEquals(t.set(["a"], 5), [{ p: ["a"], del: true }, { p: ["a"], v: 5 }]);
  assertEquals(t.set(["a", "x"], 1), [{ p: ["a"], del: true }, {
    p: ["a", "x"],
    v: 1,
  }]);
});

Deno.test("Map values keep string keys that look like integers", () => {
  const t = new StateTree();
  assertEquals(t.set(["m"], new Map([["3", 1]])), [{ p: ["m", "3"], v: 1 }]);
});

Deno.test("leaves lists every value", () => {
  const t = new StateTree();
  t.set([], { a: 1, b: { c: "x" } });
  assertEquals(t.leaves(), [{ p: ["a"], v: 1 }, { p: ["b", "c"], v: "x" }]);
  assertEquals(t.delete(["b"]), [{ p: ["b"], del: true }]);
  assertEquals(t.delete(["nope"]), []);
});

Deno.test("coalescer keeps the latest op per path, in write order", () => {
  const c = new PatchCoalescer();
  c.add({ p: ["a"], v: 1 });
  c.add({ p: ["b"], v: 1 });
  c.add({ p: ["a"], v: 2 });
  assertEquals(c.take(), [{ p: ["b"], v: 1 }, { p: ["a"], v: 2 }]);
  assertEquals(c.size, 0);
});

Deno.test("a delete drops pending ops below it but not a later set", () => {
  const c = new PatchCoalescer();
  c.add({ p: ["o", 1, "x"], v: 1 });
  c.add({ p: ["o", 2], v: 1 });
  c.add({ p: ["o"], del: true });
  c.add({ p: ["o"], v: 7 });
  assertEquals(c.take(), [{ p: ["o"], del: true }, { p: ["o"], v: 7 }]);
});
