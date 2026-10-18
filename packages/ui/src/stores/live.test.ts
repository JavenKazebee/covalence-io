import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import type { Leaf } from "../live/protocol";
import { useLiveStore } from "./live";

function leaf(path: string, value: unknown, seq = 1): Leaf {
  return { path, value, quality: "good", ts: 1_700_000_000_000, seq };
}

const paths = (store: ReturnType<typeof useLiveStore>) => [...store.leaves.keys()].sort();

describe("live store", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("holds the leaves of a snapshot", () => {
    const store = useLiveStore();
    store.applySnapshot(["vh.outputs"], [leaf("vh.outputs[0].source", 3), leaf("vh.outputs[1].source", 0)]);
    expect(store.leaf("vh.outputs[0].source")?.value).toBe(3);
    expect(paths(store)).toEqual(["vh.outputs[0].source", "vh.outputs[1].source"]);
  });

  it("replaces everything under a snapshot's paths, and nothing else", () => {
    const store = useLiveStore();
    store.applySnapshot(["vh.outputs"], [leaf("vh.outputs[0].source", 3), leaf("vh.outputs[1].source", 0)]);
    store.applySnapshot(["vh.inputs"], [leaf("vh.inputs[0].label", "Cam 1")]);
    store.applySnapshot(["vh.outputs"], [leaf("vh.outputs[0].source", 5, 2)]);
    expect(paths(store)).toEqual(["vh.inputs[0].label", "vh.outputs[0].source"]);
    expect(store.leaf("vh.outputs[0].source")).toEqual(leaf("vh.outputs[0].source", 5, 2));
  });

  it("empties a path whose snapshot has no leaves", () => {
    const store = useLiveStore();
    store.applySnapshot(["vh"], [leaf("vh.outputs[0].source", 3)]);
    store.applySnapshot(["vh"], []);
    expect(paths(store)).toEqual([]);
  });

  it("treats a prefix as whole segments", () => {
    const store = useLiveStore();
    store.applySnapshot(["vh"], [leaf("vh.out", 1), leaf("vh.outputs[0].source", 3)]);
    store.applySnapshot(["vh.out"], []);
    expect(paths(store)).toEqual(["vh.outputs[0].source"]);
  });

  it("covers the exact path and bracketed keys", () => {
    const store = useLiveStore();
    store.applySnapshot(["vh"], [leaf("vh.level", 1), leaf('vh.outputs["SDI 4"].source', 2), leaf("vh.outputs[0]", 3)]);
    store.applySnapshot(['vh.outputs["SDI 4"]', "vh.level"], []);
    expect(paths(store)).toEqual(["vh.outputs[0]"]);
  });

  it("sets and deletes leaves from a patch", () => {
    const store = useLiveStore();
    store.applySnapshot(["vh.outputs"], [leaf("vh.outputs[0].source", 3), leaf("vh.outputs[1].source", 0)]);
    store.applyPatch(
      [leaf("vh.outputs[0].source", 7, 9), leaf("vh.outputs[2].source", 1, 9)],
      ["vh.outputs[1].source", "vh.outputs[9].source"],
    );
    expect(paths(store)).toEqual(["vh.outputs[0].source", "vh.outputs[2].source"]);
    expect(store.leaf("vh.outputs[0].source")?.seq).toBe(9);
  });

  it("keeps a stale leaf's quality from a patch", () => {
    const store = useLiveStore();
    store.applyPatch([{ ...leaf("vh.outputs[0].source", 3), quality: "stale" }], []);
    expect(store.leaf("vh.outputs[0].source")?.quality).toBe("stale");
  });

  it("lists leaves under a prefix in path order", () => {
    const store = useLiveStore();
    store.applyPatch(
      [leaf("vh.outputs[10].source", 1), leaf("vh.outputs[2].source", 2), leaf("vh.inputs[0].label", "x")],
      [],
    );
    expect(store.leavesUnder("vh.outputs").map((l) => l.path)).toEqual([
      "vh.outputs[2].source",
      "vh.outputs[10].source",
    ]);
  });

  it("prunes only what no kept prefix covers", () => {
    const store = useLiveStore();
    store.applySnapshot(["vh"], [leaf("vh.outputs[0].source", 3), leaf("vh.inputs[0].label", "x")]);
    store.prune(["vh"], ["vh.outputs"]);
    expect(paths(store)).toEqual(["vh.outputs[0].source"]);
  });
});
