import { describe, expect, it } from "vitest";
import { deckControls, gridOf, missingControls, neighbour, toLayerControl, virtualDeck, type Layer } from "./model";

const layer = (controls: Layer["controls"]): Layer => ({ id: "l", name: "L", slug: "l", controls });

describe("layers", () => {
  it("copy a surface's controls with their capabilities", () => {
    const k7 = deckControls(virtualDeck)[7]!;
    expect(toLayerControl(k7)).toEqual({ id: "k7", at: { x: 2, y: 1 }, inputs: ["momentary"], outputs: ["image"] });
  });

  it("fit a surface that has each control with what it needs", () => {
    const deck = deckControls(virtualDeck);
    expect(missingControls(layer(deck.map(toLayerControl)), deck)).toEqual([]);
    // Fewer controls is fine; a missing one or a missing capability isn't.
    expect(missingControls(layer([{ id: "k0" }]), deck)).toEqual([]);
    expect(
      missingControls(layer([{ id: "k15" }, { id: "k1", inputs: ["relative"] }, { id: "k2", outputs: ["led"] }]), deck),
    ).toEqual(["k15", "k1", "k2"]);
  });

  it("are drawn where their controls are, with unplaced ones in a row below", () => {
    const { cells, cols, rows } = gridOf([
      { id: "a", at: { x: 0, y: 0, w: 2 } },
      { id: "b", at: { x: 2, y: 1 } },
      { id: "c" },
      { id: "d" },
    ]);
    expect({ cols, rows }).toEqual({ cols: 3, rows: 3 });
    expect(cells.map((c) => [c.control.id, c.x, c.y, c.w])).toEqual([
      ["a", 0, 0, 2],
      ["b", 2, 1, 1],
      ["c", 0, 2, 1],
      ["d", 1, 2, 1],
    ]);
  });

  it("move between controls by position", () => {
    const { cells } = gridOf(deckControls(virtualDeck).map(toLayerControl));
    expect(neighbour(cells, "k0", 1, 0)).toBe("k1");
    expect(neighbour(cells, "k0", 0, 1)).toBe("k5");
    expect(neighbour(cells, "k0", -1, 0)).toBeUndefined();
    expect(neighbour(cells, "k14", 0, -1)).toBe("k9");
  });
});
