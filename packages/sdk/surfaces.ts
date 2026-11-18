/**
 * Surfaces a plugin provides (plugin protocol §8): a device with controls
 * the core reads and draws on. The plugin translates; the core interprets
 * (ADR 0025). It reports what each control did, with the device's time, and
 * shows the images the core renders.
 *
 * ```ts
 * const deck = ctx.surfaces.attach({
 *   model: "elgato.mk2", serial, name: "Stream Deck MK.2",
 *   controls: presets.keys(5, 3, { w: 72, h: 72, format: "jpeg", rotation: 180 }),
 * });
 * deck.onDraw(({ control, image }) => device.fillKey(control, image));
 * device.on("down", (k) => deck.down(`k${k}`));
 * ```
 *
 * A motor fader takes its level with `onLevel`, and reports its motor's own
 * movement with `echo`, so nothing reacts to the device moving itself
 * (ADR 0035):
 *
 * ```ts
 * desk.onLevel(({ control, value }) => device.moveFader(control, value));
 * device.on("fader", (f, v) => device.touched(f) ? desk.value(f, v) : desk.echo(f, v));
 * ```
 *
 * @module
 */

import type {
  ControlDef,
  ImageFormat,
  InputCap,
  InputKind,
  OutputCap,
  Placement,
} from "@covalence-io/protocol";
import { toBytes } from "./io.ts";

/** How the runtime talks to the core: notifications only. */
export interface SurfaceSink {
  notify(method: string, params: unknown): void;
}

/** A key's or display's image output: native size, format and turn. */
export interface ImageSpec {
  w: number;
  h: number;
  format: ImageFormat;
  /** Degrees clockwise the core turns images: 180 for gen-2 Stream Decks. */
  rotation?: 0 | 90 | 180 | 270;
}

type At = Placement;
type Common = { id: string; at: At; label?: string };

/**
 * Controls by what they are, as capability lists (spec §8). A device with
 * no physical layout still places its controls: `row` puts them side by
 * side.
 */
export const presets = {
  /** A key: pressed and released, and an image if it has a display. */
  button(c: Common & { image?: ImageSpec; velocity?: boolean }): ControlDef {
    return control(c, [
      c.velocity
        ? { kind: "momentary", velocity: true }
        : { kind: "momentary" },
    ], c.image ? [image(c.image)] : []);
  },
  /** A rotary encoder: turned, and pressed if `push`. */
  encoder(c: Common & { push?: boolean }): ControlDef {
    const inputs: InputCap[] = [{ kind: "relative" }];
    if (c.push) inputs.push({ kind: "momentary" });
    return control(c, inputs, []);
  },
  /** A fader: a position from 0 to 1, motorised if `motor`. */
  fader(c: Common & { motor?: boolean }): ControlDef {
    return control(c, [{ kind: "absolute" }], [
      c.motor ? { kind: "level", motor: true } : { kind: "level" },
    ]);
  },
  /** A touch strip: a position while touched. */
  touchstrip(c: Common & { image?: ImageSpec }): ControlDef {
    return control(
      c,
      [{ kind: "absolute" }, { kind: "touch" }],
      c.image ? [image(c.image)] : [],
    );
  },
  /** A display with no input. */
  lcd(c: Common & { image: ImageSpec }): ControlDef {
    return control(c, [], [image(c.image)]);
  },
  /** A light. */
  led(c: Common & { rgb?: boolean }): ControlDef {
    return control(c, [], [
      c.rgb ? { kind: "led", rgb: true } : { kind: "led" },
    ]);
  },
  /**
   * `cols` × `rows` image keys, `k0` … left to right, top to bottom: a
   * Stream Deck's grid.
   */
  keys(cols: number, rows: number, image?: ImageSpec): ControlDef[] {
    const out: ControlDef[] = [];
    for (let i = 0; i < cols * rows; i++) {
      out.push(presets.button({
        id: `k${i}`,
        at: { x: i % cols, y: Math.floor(i / cols) },
        image,
      }));
    }
    return out;
  },
  /** Places controls without a layout side by side, in the order given. */
  row(controls: Omit<ControlDef, "at">[]): ControlDef[] {
    return controls.map((c, x) => ({ ...c, at: { x, y: 0 } }));
  },
};

function control(
  c: Common,
  inputs: InputCap[],
  outputs: OutputCap[],
): ControlDef {
  const def: ControlDef = { id: c.id, at: c.at };
  if (c.label !== undefined) def.label = c.label;
  if (inputs.length > 0) def.inputs = inputs;
  if (outputs.length > 0) def.outputs = outputs;
  return def;
}

function image(spec: ImageSpec): OutputCap {
  const out: OutputCap = {
    kind: "image",
    w: spec.w,
    h: spec.h,
    format: spec.format,
  };
  if (spec.rotation) out.rotation = spec.rotation;
  return out;
}

export interface SurfaceSpec {
  /** This connection's key for the device. Default: `model`, then `serial`. */
  key?: string;
  /** The device's model, which with `serial` finds its surface entity. */
  model: string;
  serial?: string;
  /** What a new surface entity is called. */
  name: string;
  controls: ControlDef[];
}

/** An image the core drew for a control. */
export interface Draw {
  control: string;
  image: Uint8Array;
  format: ImageFormat;
}

/** A level the core set on a control's `level` output. */
export interface Level {
  control: string;
  /** 0 to 1. */
  value: number;
}

/** A surface this connection attached. */
export class SurfaceHandle {
  readonly #sink: SurfaceSink;
  readonly #draws = new Set<(draw: Draw) => void>();
  readonly #levels = new Set<(level: Level) => void>();
  readonly #forget: () => void;
  #attached = true;

  constructor(
    readonly key: string,
    readonly spec: SurfaceSpec,
    sink: SurfaceSink,
    forget: () => void,
  ) {
    this.#sink = sink;
    this.#forget = forget;
  }

  get attached(): boolean {
    return this.#attached;
  }

  /**
   * Reports what a control did. `ts` is when the device did it, in ms; by
   * default, now. Input state, events and gestures are the core's.
   */
  input(
    control: string,
    kind: InputKind,
    value?: number | [number, number],
    ts: number = performance.timeOrigin + performance.now(),
    echo = false,
  ) {
    if (!this.#attached) return;
    const p: Record<string, unknown> = {
      surface: this.key,
      control,
      kind,
      ts,
    };
    if (value !== undefined) p.value = value;
    if (echo) p.echo = true;
    this.#sink.notify("surface.input", p);
  }

  down(control: string, velocity?: number, ts?: number) {
    this.input(control, "down", velocity, ts);
  }

  up(control: string, ts?: number) {
    this.input(control, "up", undefined, ts);
  }

  /** Signed detents since the last turn. */
  turn(control: string, detents: number, ts?: number) {
    this.input(control, "turn", detents, ts);
  }

  /** A position from 0 to 1, or `[x, y]`. */
  value(control: string, value: number | [number, number], ts?: number) {
    this.input(control, "value", value, ts);
  }

  touch(control: string, touched: boolean, ts?: number) {
    this.input(control, "touch", touched ? 1 : 0, ts);
  }

  /**
   * A position the device moved to by itself, such as a motor fader
   * following its level. The core keeps it as input state and gives no
   * events.
   */
  echo(control: string, value: number | [number, number], ts?: number) {
    this.input(control, "value", value, ts, true);
  }

  /** Calls `fn` with each image the core draws. Returns an unsubscribe. */
  onDraw(fn: (draw: Draw) => void): () => void {
    this.#draws.add(fn);
    return () => this.#draws.delete(fn);
  }

  /** Calls `fn` with each level the core sets. Returns an unsubscribe. */
  onLevel(fn: (level: Level) => void): () => void {
    this.#levels.add(fn);
    return () => this.#levels.delete(fn);
  }

  /** Detaches: the core releases its input and stops drawing. */
  detach() {
    if (!this.#attached) return;
    this.#attached = false;
    this.#sink.notify("surface.detach", { surface: this.key });
    this.#forget();
  }

  /** @internal */
  _draw(draw: Draw) {
    for (const fn of this.#draws) fn(draw);
  }

  /** @internal */
  _level(level: Level) {
    for (const fn of this.#levels) fn(level);
  }
}

/** The surfaces of one connection. The runtime makes one. */
export class Surfaces {
  readonly #handles = new Map<string, SurfaceHandle>();

  constructor(readonly sink: SurfaceSink) {}

  /**
   * Attaches a surface. Attaching the same key again replaces it, for a
   * device whose controls changed.
   */
  attach(spec: SurfaceSpec): SurfaceHandle {
    const key = spec.key ??
      (spec.serial ? `${spec.model}:${spec.serial}` : spec.model);
    this.#handles.get(key)?.detach();
    const handle = new SurfaceHandle(
      key,
      spec,
      this.sink,
      () => {
        if (this.#handles.get(key) === handle) this.#handles.delete(key);
      },
    );
    this.#handles.set(key, handle);
    const p: Record<string, unknown> = {
      surface: key,
      model: spec.model,
      name: spec.name,
      controls: spec.controls,
    };
    if (spec.serial) p.serial = spec.serial;
    this.sink.notify("surface.attach", p);
    return handle;
  }

  /** Handles `surface.draw` and `surface.level`. Returns whether it was one. */
  receive(method: string, params: unknown): boolean {
    if (method === "surface.level") {
      const p = params as { surface: string; control: string; value: number };
      this.#handles.get(p.surface)?._level({
        control: p.control,
        value: p.value,
      });
      return true;
    }
    if (method !== "surface.draw") return false;
    const p = params as {
      surface: string;
      control: string;
      image: Uint8Array | number[];
      format: ImageFormat;
    };
    this.#handles.get(p.surface)?._draw({
      control: p.control,
      image: toBytes(p.image),
      format: p.format,
    });
    return true;
  }

  /** Detaches every surface: the connection is going. */
  detachAll() {
    for (const handle of [...this.#handles.values()]) handle.detach();
  }
}
