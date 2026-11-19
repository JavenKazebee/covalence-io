import { describe, expect, it } from "vitest";
import { formOf, newSource, refsOf, type Source } from "../sources/model";
import {
  fromDraft,
  helperPath,
  isEmpty,
  isSimpleTemplate,
  literalText,
  parseLiteral,
  parseRef,
  parseTemplate,
  refText,
  templateText,
  toDraft,
  type Aliases,
  type ControlBinding,
} from "./model";

// The fixtures the Rust tests use too (overview spec: project entities): valid bindings, as stored and served.
const fixtures = import.meta.glob<ControlBinding>("../../../../crates/covalence-core/fixtures/bindings/*.json", {
  eager: true,
  import: "default",
});
const problemFixtures = import.meta.glob<{ binding: ControlBinding }>(
  "../../../../crates/covalence-core/fixtures/bindings/problems/*.json",
  { eager: true, import: "default" },
);

const VH = "01JBZ7P0D3F5G7H9J1K3L5M7N9";
const aliases: Aliases = {
  alias: (id) => (id === VH ? "vh" : undefined),
  conn: (alias) => (alias === "vh" ? VH : undefined),
};

describe("binding model", () => {
  it("has the shared fixtures", () => {
    expect(Object.keys(fixtures).length).toBeGreaterThanOrEqual(4);
    expect(Object.keys(problemFixtures).length).toBeGreaterThanOrEqual(1);
  });

  it.each(Object.entries(fixtures))("round-trips %s through the editor unchanged", (_, binding) => {
    expect(fromDraft(toDraft(binding))).toEqual(binding);
  });

  it.each(Object.entries(fixtures))("reads back every ref and template in %s as text", (_, binding) => {
    for (const b of Object.values(binding.props)) {
      if (b === undefined) continue;
      for (const ref of refsOf(b)) expect(parseRef(refText(ref, aliases), aliases)).toEqual({ ok: true, value: ref });
      if (formOf(b) === "template") {
        const parts = (b as { parts: Source[] }).parts;
        if (isSimpleTemplate(parts)) expect(parseTemplate(templateText(parts, aliases), aliases)).toEqual({ ok: true, value: parts });
      }
    }
  });

  it("round-trips problem fixtures it can't save, too", () => {
    for (const { binding } of Object.values(problemFixtures)) {
      expect(fromDraft(toDraft(binding)).props).toEqual(binding.props);
    }
  });

  it("keeps what the editor doesn't edit: other properties, helpers and gestures", () => {
    const binding: ControlBinding = {
      id: "b",
      layer: "p",
      control: "f0",
      props: { value: { ref: { conn: VH, path: ["level"] } }, label: "Vol" },
      helpers: { level: { use: "absolute", follow: { ref: { conn: VH, path: ["level"] } } } },
      gestures: { longAfter: "800ms" },
    };
    const sent = fromDraft(toDraft(binding));
    expect(sent).toEqual(binding);
    expect(Object.keys(sent.props)).toEqual(["label", "value"]);
    expect(isEmpty({ ...sent, props: {} })).toBe(false);
    expect(helperPath("cams", "f0", "level", binding.helpers!.level!)).toBe("component.cams.controls.f0.level");
    expect(helperPath("cams", "f0", "level", { use: "toggle", scope: "surface", as: "on" }, "deck")).toBe(
      "surface.deck.controls.f0.helpers.on",
    );
  });

  it("writes refs by alias, and reserved roots as they are", () => {
    expect(refText({ conn: VH, path: ["outputs", 1, "source"] }, aliases)).toBe("vh.outputs[1].source");
    expect(refText({ conn: "gone", path: ["x"] }, aliases)).toBe("gone.x");
    expect(parseRef("vars.preview", aliases)).toEqual({ ok: true, value: { root: "vars", path: ["preview"] } });
    expect(parseRef('vh.outputs["SDI 4"]', aliases)).toEqual({ ok: true, value: { conn: VH, path: ["outputs", "SDI 4"] } });
  });

  it("says why a ref doesn't read", () => {
    expect(parseRef("", aliases)).toEqual({ ok: false, error: "Required" });
    expect(parseRef("atem.program", aliases)).toEqual({ ok: false, error: "No connection atem" });
    expect(parseRef("vh.outputs[x]", aliases).ok).toBe(false);
    expect(parseRef("vh outputs", aliases).ok).toBe(false);
  });

  it("reads templates with formats and escaped braces", () => {
    expect(parseTemplate("{{Cam}} {vars.cam|upper}: {vh.level}%", aliases)).toEqual({
      ok: true,
      value: [
        "{Cam} ",
        { use: "format", input: { ref: { root: "vars", path: ["cam"] } }, format: "upper" },
        ": ",
        { ref: { conn: VH, path: ["level"] } },
        "%",
      ],
    });
    expect(templateText(["{x}"], aliases)).toBe("{{x}}");
    expect(parseTemplate('{vh.outputs["a}b"]}', aliases)).toEqual({ ok: true, value: [{ ref: { conn: VH, path: ["outputs", "a}b"] } }] });
    expect(parseTemplate("", aliases)).toEqual({ ok: true, value: [] });
  });

  it("says why a template doesn't read", () => {
    expect(parseTemplate("Out {vh.x", aliases).ok).toBe(false);
    expect(parseTemplate("Out }", aliases).ok).toBe(false);
    expect(parseTemplate("{vh.x|roman}", aliases)).toEqual({ ok: false, error: expect.stringContaining("No format roman") });
    expect(parseTemplate("{atem.x}", aliases)).toEqual({ ok: false, error: "{atem.x}: No connection atem" });
  });

  it("quotes text that would read back as something else", () => {
    for (const value of [3, 0.5, true, false, null, "red", "3", "true", " padded", '"quoted"']) {
      expect(parseLiteral(literalText(value))).toEqual(value);
    }
    expect(literalText("3")).toBe('"3"');
    expect(literalText(undefined)).toBe("");
    expect(parseLiteral(" ")).toBeUndefined();
  });

  it("leaves out properties left unbound, in the editor's order", () => {
    const draft: ControlBinding = { id: "", layer: "p", control: "k0", props: { background: newSource("expr"), label: undefined } };
    expect(Object.keys(fromDraft(draft).props)).toEqual(["background"]);
  });
});
