// Surfaces and layers (surfaces spec §1, §2). Mirrors `covalence_core::surface`
// by hand.

import type { Problem } from "../flows/model";

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
  startLayer?: string;
}

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
  outputs?: { kind: OutputKind }[];
}

/** A control a layer covers, and what it needs that control to do. */
export interface LayerControl {
  id: string;
  at?: Placement;
  inputs?: InputKind[];
  outputs?: OutputKind[];
}

export interface Layer {
  id: string;
  name: string;
  slug: string;
  controls: LayerControl[];
}

export interface SavedSurface {
  surface: Surface;
  warnings: Problem[];
}

export interface SavedLayer {
  layer: Layer;
  warnings: Problem[];
}

export interface Model {
  name: string;
  layout: Layout;
  /** Key image width and height, in pixels. */
  key: number;
}

/**
 * The surface models the UI can be. Only the virtual deck in M5; it declares
 * 144 px keys so they stay sharp on high-density screens (surfaces spec §8).
 */
export const VIRTUAL_DECK = "virtual.deck15";

export const virtualDeck: Model = { name: "Virtual Stream Deck (15 keys)", layout: { cols: 5, rows: 3 }, key: 144 };

export const models: Record<string, Model> = { [VIRTUAL_DECK]: virtualDeck };

/** A deck's keys, `k0`, `k1` … in row-major order. */
export function keysOf(layout: Layout): string[] {
  return Array.from({ length: layout.cols * layout.rows }, (_, i) => `k${i}`);
}

/** The controls a deck of `model` attaches with (surfaces spec §8). */
export function deckControls(model: Model): ControlDef[] {
  const { cols } = model.layout;
  return keysOf(model.layout).map((id, i) => ({
    id,
    at: { x: i % cols, y: Math.floor(i / cols) },
    inputs: [{ kind: "momentary" }],
    outputs: [{ kind: "image" }],
  }));
}

/** What a new layer copies from a surface's control. */
export function toLayerControl(def: ControlDef): LayerControl {
  return {
    id: def.id,
    at: { ...def.at },
    inputs: (def.inputs ?? []).map((i) => i.kind),
    outputs: (def.outputs ?? []).map((o) => o.kind),
  };
}

/**
 * The layer's controls a surface with `controls` lacks or can't support
 * (surfaces spec §5.4). Empty when the layer fits.
 */
export function missingControls(layer: Layer, controls: ControlDef[]): string[] {
  return layer.controls
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
  control: LayerControl;
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Where the editor draws each control: at its `at`, or for one without, in a
 * row below the rest. Columns and rows are the extent of them all.
 */
export function gridOf(controls: LayerControl[]): { cells: Cell[]; cols: number; rows: number } {
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

export const gestures = ["press", "release", "double", "long", "hold"] as const;
export type Gesture = (typeof gestures)[number] | "turn";

export const gestureHelp: Record<(typeof gestures)[number], string> = {
  press: "Pressed",
  release: "Released",
  double: "Double-pressed",
  long: "Held for half a second",
  hold: "Held, repeating",
};
