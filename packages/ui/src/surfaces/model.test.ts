import { describe, expect, it } from "vitest";
import {
  candidatesText,
  deckControls,
  eventsFor,
  gridOf,
  missingControls,
  neighbour,
  presetOf,
  presetWhen,
  shiftKey,
  toLayerControl,
  virtualDeck,
  whenExpr,
  type Layer,
} from "./model";

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

  it("offer the events a control's inputs can give", () => {
    expect(eventsFor({ id: "k0", inputs: ["momentary"] })).toEqual(["press", "release", "double", "long", "hold"]);
    expect(eventsFor({ id: "dial", inputs: ["relative", "momentary"] })).toContain("turn");
    expect(eventsFor({ id: "fader", inputs: ["absolute", "touch"] })).toEqual(["change", "touch", "untouch"]);
    expect(eventsFor({ id: "led" })).toEqual([]);
  });
});

describe("layer fields", () => {
  const none = { alias: () => undefined, conn: () => undefined };
  const keys = (ids: string[]) => ids.map((id) => ({ id }));

  it("read their when as a preset and back", () => {
    expect(presetOf(undefined)).toBe("nav");
    expect(presetOf({ nav: "popover" })).toBe("popover");
    expect(presetOf(presetWhen("shift", "k15"))).toBe("shift");
    expect(shiftKey(presetWhen("shift", "k15"))).toBe("k15");
    expect(presetOf({ while: { expr: "self.controls.k15.pressed && vars.armed" } })).toBe("while");
    expect(presetOf({ always: true })).toBe("always");
  });

  it("show the expression a preset compiles to", () => {
    expect(whenExpr(undefined, "cams", none)).toBe('self.current.main == "cams"');
    expect(whenExpr({ always: true }, "strip", none)).toBe("true");
    expect(whenExpr(presetWhen("shift", "k15"), "shift", none)).toBe("self.controls.k15.pressed");
  });

  it("list a control's candidates, highest first", () => {
    const cams: Layer = { id: "c", name: "Cameras", slug: "cams", controls: keys(["k3", "k4"]) };
    const shift: Layer = {
      id: "s",
      name: "Shift",
      slug: "shift",
      controls: keys(["k3"]),
      level: "over",
      when: presetWhen("shift", "k15"),
    };
    const strip: Layer = { id: "t", name: "Strip", slug: "strip", controls: keys(["k3"]), level: "under", rank: 50, when: { always: true } };
    expect(candidatesText("k3", [strip, cams, shift])).toBe(
      "k3: Shift (over, while k15 is held), otherwise Cameras (main), otherwise Strip (under, always)",
    );
    expect(candidatesText("k9", [cams])).toBe("k9: no layer covers it");
  });
});
