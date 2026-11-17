import { describe, expect, it } from "vitest";
import {
  childrenOf,
  formOf,
  literal,
  literalOf,
  newSource,
  refsOf,
  toExpr,
  uses,
  withFallback,
  type Aliases,
  type Form,
  type Source,
} from "./model";

const VH = "01JBZ7P0D3F5G7H9J1K3L5M7N9";
const aliases: Aliases = {
  alias: (id) => (id === VH ? "vh" : undefined),
  conn: (alias) => (alias === "vh" ? VH : undefined),
};

const fixtures = import.meta.glob<{ props: Record<string, Source> }>("../../../../crates/covalence-core/fixtures/bindings/*.json", {
  eager: true,
  import: "default",
});

describe("sources", () => {
  it("recognises each form by shape, as the core does", () => {
    expect(formOf("red")).toBe("literal");
    expect(formOf({ a: 1 })).toBe("literal");
    expect(formOf({ value: { ref: 1 } })).toBe("literal");
    expect(literalOf({ value: { ref: 1 } })).toEqual({ ref: 1 });
    expect(formOf({ ref: { root: "vars", path: [] } })).toBe("ref");
    expect(formOf({ expr: "1" })).toBe("expr");
    // `use` decides, even beside an option named `value`.
    expect(formOf({ use: "compare", input: 1, op: "==", value: 2 })).toBe("compare");
  });

  it("escapes a literal that would read as a source", () => {
    expect(literal({ expr: "x" })).toEqual({ value: { expr: "x" } });
    expect(literal("x")).toBe("x");
  });

  it("starts every form in its own shape", () => {
    const forms: Form[] = ["literal", "ref", "expr", ...uses];
    for (const form of forms) expect(formOf(newSource(form))).toBe(form);
  });

  it("sets and clears fallbacks, never on a literal", () => {
    const s = withFallback({ ref: { root: "vars", path: ["x"] } }, "?");
    expect(s).toEqual({ ref: { root: "vars", path: ["x"] }, fallback: "?" });
    expect(withFallback(s, undefined)).toEqual({ ref: { root: "vars", path: ["x"] } });
    expect(withFallback("lit", "?")).toBe("lit");
  });

  it("finds the paths nested sources read", () => {
    const tally = Object.entries(fixtures).find(([name]) => name.endsWith("tally.json"))![1];
    expect(refsOf(tally.props.background!)).toEqual([
      { conn: VH, path: ["outputs", 1, "source"] },
      { root: "vars", path: ["preview"] },
    ]);
    expect(childrenOf({ use: "not", input: true })).toEqual([true]);
  });

  it("compiles to the expressions the core writes", () => {
    const tally = Object.entries(fixtures).find(([name]) => name.endsWith("tally.json"))![1];
    // `compiled_expressions_read_like_the_spec` in crates/covalence-core/tests/bindings.rs.
    expect(toExpr(tally.props.background!, aliases)).toBe(
      '(vh.outputs[1].source === 3) ? "#e02020" : (vars.preview === 3) ? "#20a040" : "#333333"',
    );
    expect(toExpr(tally.props.label!, { alias: () => undefined, conn: () => undefined })).toBeNull();
    expect(toExpr({ use: "all", of: [] }, aliases)).toBe("true");
    expect(toExpr({ use: "not", input: { ref: { root: "vars", path: ["m"] } } }, aliases)).toBe("!vars.m");
    expect(toExpr({ use: "format", input: { ref: { root: "vars", path: ["t"] } }, format: "mmss" }, aliases)).toBe(
      "((v) => v === undefined ? undefined : mmss(v))(vars.t)",
    );
  });
});
