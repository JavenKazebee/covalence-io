// Surfaces and components (surfaces spec §1, §2). Mirrors `covalence_core::surface`
// by hand.

import type { ControlBinding, RuntimeProblem } from "../bindings/model";
import type { Problem } from "../flows/model";
import { toExpr, type Aliases, type Source } from "../sources/model";

export interface Layout {
  cols: number;
  rows: number;
}

export interface Surface {
  /** Empty in a create request. */
  id: string;
  name: string;
  /** Derived from `name` by the server when empty. */
  slug: string;
  model: string;
  serial?: string;
  startComponent?: string;
  /** The connection whose device this is, for a surface a plugin provides. */
  provider?: string;
  /** A virtual deck's encoders and faders, beside its keys (surfaces spec §11.4). */
  virtual?: VirtualLayout;
}

/** What a virtual deck has besides its keys. Mirrors `covalence_core::surface::VirtualLayout`. */
export interface VirtualLayout {
  /** `e0`…`e3`, a row beneath the keys. Default 0. */
  encoders?: number;
  /** `f0`…`f3`, to the right of the keys, as tall as the deck. Default 0. */
  faders?: number;
  /** Whether the faders' level moves them. Default true. */
  motor?: boolean;
}

/** The most encoders, or faders, a virtual deck can have. */
export const MAX_VIRTUAL = 4;

/** Where a control is, in grid cells. `w` and `h` default to 1. */
export interface Placement {
  x: number;
  y: number;
  w?: number;
  h?: number;
}

export type InputKind = "momentary" | "relative" | "absolute" | "touch";
export type OutputKind = "image" | "led" | "level" | "text";

/** A control as a surface declares it when it attaches (plugin protocol §8). */
export interface ControlDef {
  id: string;
  label?: string;
  at: Placement;
  inputs?: { kind: InputKind }[];
  outputs?: { kind: OutputKind; motor?: boolean }[];
}

/** A control a component covers, and what it needs that control to do. */
export interface ControlSpec {
  id: string;
  at?: Placement;
  inputs?: InputKind[];
  outputs?: OutputKind[];
}

/**
 * Whatever is on a set of controls (composition spec §7): its controls, the
 * bindings of each, and, when it's shown on surfaces, `show`. Mirrors
 * `covalence_core::component` by hand.
 */
export interface Component {
  /** Empty in a create request. */
  id: string;
  name: string;
  /** Derived from `name` by the server when empty. */
  slug: string;
  controls: ControlSpec[];
  /** One per bound control, with IDs unique in the component. */
  bindings?: ControlBinding[];
  /** Shown on surfaces (composition spec §7.3); absent, it's only used inside others. */
  show?: Show;
}

/** Where and when a component is shown (surfaces spec §5.2, §5.3). */
export interface Show {
  /** The surfaces it may appear on, by ID; every surface when absent (surfaces spec §1). */
  on?: string | string[] | { expr: string } | { any: unknown[] } | { all: unknown[] } | { not: unknown };
  /** When it's active; `{nav: "main"}` when absent (surfaces spec §5.3). */
  when?: When;
  /** Its band; `normal` when absent. */
  level?: Level;
  /** Its order within its level, 0 to 99. */
  rank?: Source;
  /** `through`: sets only what it binds, the rest from the components below (surfaces spec §5.2.1). */
  cascade?: "replace" | "through";
}

/** `GET /components/problems`: a failing `show` field or binding property. */
export interface ComponentProblem {
  component: string;
  /** `["show", <field>]` or `["bindings", <id>, "props", <prop>]`. */
  field: string[];
  message: string;
}

/** A component's runtime problems with its binding properties, by binding ID. */
export function bindingRuntime(problems: ComponentProblem[], component: string): RuntimeProblem[] {
  return problems
    .filter((p) => p.component === component && p.field[0] === "bindings" && p.field[2] === "props")
    .map((p) => ({ binding: p.field[1]!, prop: p.field[3] as RuntimeProblem["prop"], message: p.message }));
}

/** A new binding's ID, unique in its component: the lowest free `b<n>`. */
export function newBindingId(c: Component): string {
  const taken = new Set((c.bindings ?? []).map((b) => b.id));
  for (let n = 1; ; n++) {
    const id = `b${n}`;
    if (!taken.has(id)) return id;
  }
}

/** A component's problems under one of its bindings, relative to it. */
export function bindingProblems<P extends Problem>(problems: P[], index: number): P[] {
  return problems
    .filter((p) => p.path[0] === "bindings" && p.path[1] === index)
    .map((p) => ({ ...p, path: p.path.slice(2) }));
}

/** When a component is active (surfaces spec §5.3). Mirrors `covalence_core::surface::When`. */
export type When = { nav: string } | { always: true } | { while: Source };

export const levels = ["under", "normal", "over"] as const;
export type Level = (typeof levels)[number];

/** The navigation group every surface has. */
export const MAIN_GROUP = "main";

/** The structures the editor offers for `when` (surfaces spec §5.3). */
export type Preset = "nav" | "popover" | "shift" | "while" | "always";

export const presetNames: Record<Preset, string> = {
  nav: "Navigation group",
  popover: "Popover",
  shift: "Shift (while a key is held)",
  while: "While a condition holds",
  always: "Always on",
};

const SHIFT = /^self\.controls\.([A-Za-z0-9_.-]+)\.pressed$/;

/** The key a shift's `when` holds, if it's the shift preset. */
export function shiftKey(when: When | undefined): string | undefined {
  if (when === undefined || !("while" in when)) return undefined;
  const w = when.while;
  if (typeof w !== "object" || w === null || Array.isArray(w) || !("expr" in w)) return undefined;
  return SHIFT.exec(String(w.expr).trim())?.[1];
}

/** Which preset a `when` is. */
export function presetOf(when: When | undefined): Preset {
  if (when === undefined || "nav" in when) return when === undefined || when.nav !== "popover" ? "nav" : "popover";
  if ("always" in when) return "always";
  return shiftKey(when) !== undefined ? "shift" : "while";
}

/** A fresh `when` for a preset. A popover or shift goes `over` by default. */
export function presetWhen(preset: Preset, key = "k0"): When {
  switch (preset) {
    case "nav":
      return { nav: MAIN_GROUP };
    case "popover":
      return { nav: "popover" };
    case "shift":
      return { while: { expr: `self.controls.${key}.pressed` } };
    case "while":
      return { while: { ref: { root: "vars", path: [] } } };
    case "always":
      return { always: true };
  }
}

/** The expression a `when` compiles to, as the editor shows it (surfaces spec §5.3). */
export function whenExpr(when: When | undefined, slug: string, aliases: Aliases): string | null {
  const w = when ?? { nav: MAIN_GROUP };
  if ("nav" in w) return `self.current.${w.nav} == ${JSON.stringify(slug)}`;
  if ("always" in w) return "true";
  return toExpr(w.while, aliases);
}

/** "while k15 is held", "in main", "always": how a `when` reads. */
export function whenText(when: When | undefined): string {
  const w = when ?? { nav: MAIN_GROUP };
  if ("nav" in w) return w.nav === MAIN_GROUP ? "main" : `in ${w.nav}`;
  if ("always" in w) return "always";
  const key = shiftKey(w);
  return key !== undefined ? `while ${key} is held` : "while its condition holds";
}

/** A rank that's a literal number, or null when it follows state. */
export function literalRank(rank: Source | undefined): number | null {
  if (rank === undefined) return 0;
  return typeof rank === "number" ? rank : null;
}

/**
 * The shown components that could control a control, highest first, as far
 * as their literal fields tell: by level, then literal rank (a computed one
 * counts as 0). Recency isn't known ahead, so ties keep the order given.
 */
export function candidates(control: string, components: readonly Component[]): Component[] {
  const order = (c: Component) => levels.indexOf(c.show?.level ?? "normal") * 100 + (literalRank(c.show?.rank) ?? 0);
  return components
    .filter((c) => c.show !== undefined && c.controls.some((k) => k.id === control))
    .map((l, i) => ({ l, i }))
    .sort((a, b) => order(b.l) - order(a.l) || a.i - b.i)
    .map(({ l }) => l);
}

/** "k3: Shift (over, while k15 is held), otherwise Cameras (main)". */
export function candidatesText(control: string, components: readonly Component[]): string {
  const found = candidates(control, components);
  if (found.length === 0) return `${control}: nothing shown covers it`;
  const one = (c: Component) => {
    const level = c.show?.level !== undefined && c.show.level !== "normal" ? `${c.show.level}, ` : "";
    const through = c.show?.cascade === "through" ? "through, " : "";
    return `${c.name} (${level}${through}${whenText(c.show?.when)})`;
  };
  return `${control}: ${found.map(one).join(", otherwise ")}`;
}

export interface SavedSurface {
  surface: Surface;
  warnings: Problem[];
}

export interface SavedComponent {
  component: Component;
  warnings: Problem[];
}

export interface Model {
  name: string;
  layout: Layout;
  /** Key image width and height, in pixels. */
  key: number;
}

/**
 * The surface models the UI can be. Only the virtual deck; it declares
 * 144 px keys so they stay sharp on high-density screens (surfaces spec §8).
 */
export const VIRTUAL_DECK = "virtual.deck15";

export const virtualDeck: Model = { name: "Virtual Stream Deck (15 keys)", layout: { cols: 5, rows: 3 }, key: 144 };

export const models: Record<string, Model> = { [VIRTUAL_DECK]: virtualDeck };

/** A deck's keys, `k0`, `k1` … in row-major order. */
export function keysOf(layout: Layout): string[] {
  return Array.from({ length: layout.cols * layout.rows }, (_, i) => `k${i}`);
}

/**
 * The controls a deck of `model` attaches with (surfaces spec §8): its keys,
 * then `layout`'s encoders in a row beneath them and faders to their right
 * (surfaces spec §11.4). The core builds the same list (`virtual_controls`).
 */
export function deckControls(model: Model, layout: VirtualLayout = {}): ControlDef[] {
  const { cols, rows } = model.layout;
  const keys: ControlDef[] = keysOf(model.layout).map((id, i) => ({
    id,
    at: { x: i % cols, y: Math.floor(i / cols) },
    inputs: [{ kind: "momentary" }],
    outputs: [{ kind: "image" }],
  }));
  const count = (n: number | undefined) => Math.max(0, Math.min(MAX_VIRTUAL, n ?? 0));
  const encoders: ControlDef[] = Array.from({ length: count(layout.encoders) }, (_, i) => ({
    id: `e${i}`,
    at: { x: i, y: rows },
    inputs: [{ kind: "relative" }, { kind: "momentary" }],
    outputs: [{ kind: "level" }],
  }));
  const motor = layout.motor ?? true;
  const faders: ControlDef[] = Array.from({ length: count(layout.faders) }, (_, i) => ({
    id: `f${i}`,
    at: { x: cols + i, y: 0, h: rows },
    inputs: [{ kind: "absolute" }, { kind: "touch" }],
    outputs: [motor ? { kind: "level", motor: true } : { kind: "level" }],
  }));
  return [...keys, ...encoders, ...faders];
}

/** The controls a virtual surface attaches with, or `undefined` for a model the browser can't be. */
export function surfaceControls(surface: Pick<Surface, "model" | "virtual">): ControlDef[] | undefined {
  const model = models[surface.model];
  return model === undefined ? undefined : deckControls(model, surface.virtual);
}

/** What a new page copies from a surface's control. */
export function toControlSpec(def: ControlDef): ControlSpec {
  return {
    id: def.id,
    at: { ...def.at },
    inputs: (def.inputs ?? []).map((i) => i.kind),
    outputs: (def.outputs ?? []).map((o) => o.kind),
  };
}

/**
 * The component's controls a surface with `controls` lacks or can't support
 * (surfaces spec §5.4). Empty when it fits.
 */
export function missingControls(component: Pick<Component, "controls">, controls: ControlDef[]): string[] {
  return component.controls
    .filter((needed) => {
      const there = controls.find((c) => c.id === needed.id);
      if (there === undefined) return true;
      const inputs = new Set((there.inputs ?? []).map((i) => i.kind));
      const outputs = new Set((there.outputs ?? []).map((o) => o.kind));
      return !(needed.inputs ?? []).every((k) => inputs.has(k)) || !(needed.outputs ?? []).every((k) => outputs.has(k));
    })
    .map((c) => c.id);
}

export interface Cell {
  control: ControlSpec;
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Where the editor draws each control: at its `at`, or for one without, in a
 * row below the rest. Columns and rows are the extent of them all.
 */
export function gridOf(controls: ControlSpec[]): { cells: Cell[]; cols: number; rows: number } {
  const placed = controls.filter((c) => c.at !== undefined);
  const below = placed.reduce((n, c) => Math.max(n, c.at!.y + (c.at!.h ?? 1)), 0);
  let x = 0;
  const cells = controls.map((control) =>
    control.at === undefined
      ? { control, x: x++, y: below, w: 1, h: 1 }
      : { control, x: control.at.x, y: control.at.y, w: control.at.w ?? 1, h: control.at.h ?? 1 },
  );
  const cols = cells.reduce((n, c) => Math.max(n, c.x + c.w), 0);
  const rows = cells.reduce((n, c) => Math.max(n, c.y + c.h), 0);
  return { cells, cols, rows };
}

/** The nearest control from `from` in a direction, for arrow keys in the editor's grid. */
export function neighbour(cells: Cell[], from: string, dx: number, dy: number): string | undefined {
  const here = cells.find((c) => c.control.id === from);
  if (here === undefined) return undefined;
  let best: { id: string; score: number } | undefined;
  for (const c of cells) {
    const ax = c.x - here.x;
    const ay = c.y - here.y;
    // Ahead in the direction, nearest first, then most in line.
    const ahead = dx !== 0 ? ax * dx : ay * dy;
    const across = dx !== 0 ? Math.abs(ay) : Math.abs(ax);
    if (c === here || ahead <= 0) continue;
    const score = ahead * 100 + across;
    if (best === undefined || score < best.score) best = { id: c.control.id, score };
  }
  return best?.id;
}

/** What a control trigger can listen for (surfaces spec §4.2). */
export const controlEvents = ["press", "release", "double", "long", "hold", "turn", "change", "touch", "untouch"] as const;
export type ControlEvent = (typeof controlEvents)[number];

export const eventHelp: Record<ControlEvent, string> = {
  press: "Pressed",
  release: "Released",
  double: "Double-pressed",
  long: "Held for half a second",
  hold: "Held, repeating",
  turn: "Turned",
  change: "Moved",
  touch: "Touched",
  untouch: "Let go",
};

/** The input a control needs to give each event. */
const eventInput: Record<ControlEvent, InputKind> = {
  press: "momentary",
  release: "momentary",
  double: "momentary",
  long: "momentary",
  hold: "momentary",
  turn: "relative",
  change: "absolute",
  touch: "touch",
  untouch: "touch",
};

/** The events a component's control can give, from the inputs it lists for it. */
export function eventsFor(control: ControlSpec | undefined): ControlEvent[] {
  const inputs = new Set(control?.inputs ?? []);
  return controlEvents.filter((e) => inputs.has(eventInput[e]));
}
