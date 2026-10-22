// Surfaces and pages (surfaces spec §1, §2). Mirrors `covalence_core::surface`
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
  startPage?: string;
}

export interface Page {
  id: string;
  name: string;
  slug: string;
  layout: Layout;
}

export interface SavedSurface {
  surface: Surface;
  warnings: Problem[];
}

export interface SavedPage {
  page: Page;
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
 * 144 px keys so they stay sharp on high-density screens (surfaces spec §4).
 */
export const models: Record<string, Model> = {
  "virtual.deck15": { name: "Virtual Stream Deck (15 keys)", layout: { cols: 5, rows: 3 }, key: 144 },
};

export const VIRTUAL_DECK = "virtual.deck15";

/** `k0`, `k1` … in row-major order (surfaces spec §1). */
export function controlsOf(layout: Layout): string[] {
  return Array.from({ length: layout.cols * layout.rows }, (_, i) => `k${i}`);
}

export const sameLayout = (a: Layout, b: Layout) => a.cols === b.cols && a.rows === b.rows;

export const layoutText = (layout: Layout) => `${layout.cols} × ${layout.rows}`;

export const gestures = ["press", "release", "double", "long", "hold"] as const;
export type Gesture = (typeof gestures)[number] | "turn";

export const gestureHelp: Record<(typeof gestures)[number], string> = {
  press: "Pressed",
  release: "Released",
  double: "Double-pressed",
  long: "Held for half a second",
  hold: "Held, repeating",
};
