import { describe, expect, it } from "vitest";
import {
  bindingProblems,
  bindingRuntime,
  bindingsOf,
  componentOf,
  layerOf,
  layerProblems,
  layerRuntime,
  newBindingId,
  type Component,
} from "./component";

const cams: Component = {
  id: "C",
  name: "Cameras",
  slug: "cams",
  controls: [{ id: "k0", outputs: ["image"] }],
  bindings: [{ id: "b1", control: "k0", props: { label: "Hi" } }],
  show: { level: "over", when: { nav: "main" } },
};

describe("components as the editors' layers and bindings", () => {
  it("flattens show into a layer, and back", () => {
    const layer = layerOf(cams);
    expect(layer).toEqual({
      id: "C",
      name: "Cameras",
      slug: "cams",
      controls: cams.controls,
      level: "over",
      when: { nav: "main" },
    });
    expect(componentOf(layer, cams.bindings!)).toEqual(cams);
    expect(componentOf({ ...layer, level: undefined, when: undefined }, []).show).toEqual({});
  });

  it("gives bindings their component as layer, and stores them without it", () => {
    const [b] = bindingsOf(cams);
    expect(b).toEqual({ id: "b1", layer: "C", control: "k0", props: { label: "Hi" } });
    expect(componentOf(layerOf(cams), [b!]).bindings).toEqual(cams.bindings);
    expect(newBindingId(cams)).toBe("b2");
    expect(newBindingId({ ...cams, bindings: [{ id: "b2", control: "k0", props: {} }] })).toBe("b1");
  });

  it("maps a component's problems onto the layer or one binding", () => {
    const problems = [
      { path: ["show", "when", "nav"], code: "invalid", message: "" },
      { path: ["controls", 0, "id"], code: "duplicateControl", message: "" },
      { path: ["bindings", 1, "props", "label"], code: "noOutput", message: "" },
    ];
    expect(layerProblems(problems).map((p) => p.path)).toEqual([["when", "nav"], ["controls", 0, "id"]]);
    expect(bindingProblems(problems, 1).map((p) => p.path)).toEqual([["props", "label"]]);
    expect(bindingProblems(problems, 0)).toEqual([]);
  });

  it("splits runtime problems into show fields and binding properties", () => {
    const runtime = [
      { component: "C", field: ["show", "rank"], message: "not a number" },
      { component: "C", field: ["bindings", "b1", "props", "label"], message: "boom" },
    ];
    expect(layerRuntime(runtime)).toEqual([{ layer: "C", field: "rank", message: "not a number" }]);
    expect(bindingRuntime(runtime)).toEqual([{ binding: "b1", prop: "label", message: "boom" }]);
  });
});
