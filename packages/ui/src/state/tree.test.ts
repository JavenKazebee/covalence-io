import { describe, expect, it } from "vitest";
import type { Leaf, Quality } from "../live/protocol";
import { buildTree, visibleRows } from "./tree";

const leaf = (path: string, value: unknown, quality: Quality = "good"): Leaf => ({
  path,
  value,
  quality,
  ts: 0,
  seq: 1,
});

const leaves = [
  leaf("vh.outputs[10].source", 2),
  leaf("vh.model", "Videohub"),
  leaf("vh.outputs[2].source", 1, "stale"),
  leaf("vh.outputs[2].name", "Output 3"),
];

describe("state tree", () => {
  it("nests leaves by segment, in path order", () => {
    const tree = buildTree("vh", leaves);
    const rows = visibleRows(tree, new Set()).map((r) => [r.depth, r.node.label, r.node.leaf?.value]);
    expect(rows).toEqual([
      [0, "model", "Videohub"],
      [0, "outputs", undefined],
      [1, "[2]", undefined],
      [2, "name", "Output 3"],
      [2, "source", 1],
      [1, "[10]", undefined],
      [2, "source", 2],
    ]);
  });

  it("gives a branch the worst quality below it", () => {
    const tree = buildTree("vh", leaves);
    const outputs = tree.children.find((c) => c.label === "outputs")!;
    expect(outputs.quality).toBe("stale");
    expect(outputs.count).toBe(3);
    expect(outputs.children.map((c) => c.quality)).toEqual(["stale", "good"]);
  });

  it("hides what's under a collapsed node", () => {
    const tree = buildTree("vh", leaves);
    expect(visibleRows(tree, new Set(["vh.outputs"])).map((r) => r.node.path)).toEqual(["vh.model", "vh.outputs"]);
  });

  it("roots at a deeper path", () => {
    const tree = buildTree("vh.outputs", leaves.filter((l) => l.path.startsWith("vh.outputs")));
    expect(tree.children.map((c) => c.path)).toEqual(["vh.outputs[2]", "vh.outputs[10]"]);
  });
});
