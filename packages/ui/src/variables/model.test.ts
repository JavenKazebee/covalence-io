import { describe, expect, it } from "vitest";
import { describeSink, kindOf, reducerOf, reducerOptions, reducerSource, varsRef, withKind, type Variable } from "./model";

const v = (fields: Partial<Variable>): Variable => ({ id: "v", name: "Armed", slug: "armed", ...fields });

describe("variables", () => {
  it("are writable, computed or accumulated by their value", () => {
    expect(kindOf(v({ initial: false }))).toBe("writable");
    expect(kindOf(v({ value: { expr: "atem.program == 3" } }))).toBe("computed");
    expect(kindOf(v({ value: { use: "toggle@1", on: { from: "x" } } }))).toBe("accumulated");
    expect(reducerOf({ use: "rules", rules: [], else: null })).toBeUndefined();
  });

  it("drop what another kind doesn't use", () => {
    const computed = withKind(v({ initial: 3, persist: true }), "computed");
    expect(computed).toEqual({ id: "v", name: "Armed", slug: "armed", persist: true, value: { expr: "" } });
    expect(withKind(computed, "writable")).toEqual({ id: "v", name: "Armed", slug: "armed", persist: true });
    expect(kindOf(withKind(computed, "accumulated"))).toBe("accumulated");
  });

  it("edit a reducer's options apart from its name", () => {
    const source = { use: "position", input: { ref: { root: "surface", path: ["deck"] } }, max: 1 };
    expect(reducerOptions(source)).toEqual({ input: source.input, max: 1 });
    expect(reducerSource("counter", { step: 2 })).toEqual({ step: 2, use: "counter" });
  });

  it("describe their sinks", () => {
    const alias = (id: string) => (id === "01VH" ? "vh" : id);
    expect(describeSink({ set: { root: "vars", path: ["live"] } }, alias)).toBe("sets vars.live");
    expect(describeSink({ action: { target: { conn: "01VH" }, action: "setVolume", param: "level" } }, alias)).toBe(
      "vh.setVolume(level), at most every 20ms",
    );
    expect(varsRef("vars.live")).toEqual({ root: "vars", path: ["live"] });
    expect(varsRef("vh.x")).toBeNull();
  });
});
