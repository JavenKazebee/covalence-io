// Components (composition spec §7): what's on a set of controls, with its
// bindings embedded and, when shown on surfaces, a `show`. Mirrors
// `covalence_core::component` by hand.
//
// The editors still edit a layer and its bindings one at a time; these
// functions turn a component into those and back, so saving a layer or a
// binding puts its component back whole.

import type { ControlBinding, RuntimeProblem } from "../bindings/model";
import type { Problem } from "../flows/model";
import type { Layer, LayerControl } from "./model";

export interface Component {
  /** Empty in a create request. */
  id: string;
  name: string;
  /** Derived from `name` by the server when empty. */
  slug: string;
  controls: LayerControl[];
  /** Unique by `id` in the component; stored without `layer`. */
  bindings?: Omit<ControlBinding, "layer">[];
  /** Shown on surfaces (composition spec §7.3); absent, it's only used inside others. */
  show?: Show;
}

export type Show = Pick<Layer, "on" | "when" | "level" | "rank" | "cascade">;

export interface SavedComponent {
  component: Component;
  warnings: Problem[];
}

/** `GET /components/problems`: a failing `show` field or binding property. */
export interface ComponentProblem {
  component: string;
  /** `["show", <field>]` or `["bindings", <id>, "props", <prop>]`. */
  field: string[];
  message: string;
}

const showFields = ["on", "when", "level", "rank", "cascade"] as const;

/** The layer the editors edit: its controls, and `show`'s fields beside them. */
export function layerOf(c: Component): Layer {
  const layer: Layer = { id: c.id, name: c.name, slug: c.slug, controls: c.controls };
  for (const f of showFields) {
    const v = c.show?.[f];
    if (v !== undefined) (layer as unknown as Record<string, unknown>)[f] = v;
  }
  return layer;
}

/** A component from a layer and its bindings: shown, since every layer is. */
export function componentOf(layer: Layer, bindings: Omit<ControlBinding, "layer">[]): Component {
  const show: Show = {};
  for (const f of showFields) {
    const v = layer[f];
    if (v !== undefined) (show as Record<string, unknown>)[f] = v;
  }
  const c: Component = { id: layer.id, name: layer.name, slug: layer.slug, controls: layer.controls, show };
  if (bindings.length > 0) c.bindings = bindings.map(stored);
  return c;
}

/** A component's bindings as the editors have them: each naming its component as `layer`. */
export function bindingsOf(c: Component): ControlBinding[] {
  return (c.bindings ?? []).map((b) => ({ ...b, layer: c.id }));
}

/** A binding as its component stores it: without `layer`. */
export function stored(b: Omit<ControlBinding, "layer"> & { layer?: string }): Omit<ControlBinding, "layer"> {
  const rest = { ...b };
  delete rest.layer;
  return rest;
}

/** A new binding's ID, unique in its component: the lowest free `b<n>`. */
export function newBindingId(c: Component): string {
  const taken = new Set((c.bindings ?? []).map((b) => b.id));
  for (let n = 1; ; n++) {
    const id = `b${n}`;
    if (!taken.has(id)) return id;
  }
}

/**
 * A component's problems as the layer editor shows them: those under `show`
 * without it. Problems under `bindings` aren't the layer's, and are left out.
 */
export function layerProblems(problems: Problem[]): Problem[] {
  return problems
    .filter((p) => p.path[0] !== "bindings")
    .map((p) => (p.path[0] === "show" ? { ...p, path: p.path.slice(1) } : p));
}

/** A component's problems as one binding's editor shows them: those under it, relative to it. */
export function bindingProblems(problems: Problem[], index: number): Problem[] {
  return problems
    .filter((p) => p.path[0] === "bindings" && p.path[1] === index)
    .map((p) => ({ ...p, path: p.path.slice(2) }));
}

/** The runtime problems of `show` fields, as the layer editor reads them. */
export function layerRuntime(problems: ComponentProblem[]): { layer: string; field: string; message: string }[] {
  return problems
    .filter((p) => p.field[0] === "show" && p.field[1] !== undefined)
    .map((p) => ({ layer: p.component, field: p.field[1]!, message: p.message }));
}

/** The runtime problems of binding properties, by binding ID. */
export function bindingRuntime(problems: ComponentProblem[]): RuntimeProblem[] {
  return problems
    .filter((p) => p.field[0] === "bindings" && p.field[2] === "props")
    .map((p) => ({ binding: p.field[1]!, prop: p.field[3] as RuntimeProblem["prop"], message: p.message }));
}
