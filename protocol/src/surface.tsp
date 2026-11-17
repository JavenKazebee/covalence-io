import "@typespec/json-schema";
import "./io.tsp";

using TypeSpec.JsonSchema;

/**
 * Surfaces: two-way devices whose controls' input the core keeps as state
 * and whose outputs it sets. Spec §8 (ADR 0024, ADR 0025).
 *
 * From the plugin host, the envelope's `conn` names the connection that
 * provides the surface, and `surface` is the host's own key for it.
 */
@jsonSchema
namespace Covalence.Protocol;

/** Where a control is, in grid cells. */
model Placement {
  x: safeint;
  y: safeint;
  w?: safeint;
  h?: safeint;
}

model MomentaryInput {
  kind: "momentary";
  velocity?: boolean;
}

model RelativeInput {
  kind: "relative";
}

model AbsoluteInput {
  kind: "absolute";
  axes?: 1 | 2;
}

model TouchInput {
  kind: "touch";
}

/** What a control can send. */
union InputCap {
  MomentaryInput,
  RelativeInput,
  AbsoluteInput,
  TouchInput,
}

model ImageOutput {
  kind: "image";
  w: safeint;
  h: safeint;
  format: string;
  rotation?: 0 | 90 | 180 | 270;
}

model LedOutput {
  kind: "led";
  rgb?: boolean;
}

model LevelOutput {
  kind: "level";
  motor?: boolean;
}

model TextOutput {
  kind: "text";
  chars?: safeint;
  lines?: safeint;
}

/** What a control can show. */
union OutputCap {
  ImageOutput,
  LedOutput,
  LevelOutput,
  TextOutput,
}

/** One control on a surface. */
model ControlDef {
  /** Unique on the surface: `k0`, `dial1`, `pad.36`. */
  id: string;

  label?: string;
  at: Placement;
  inputs?: InputCap[];
  outputs?: OutputCap[];
}

model SurfaceOutputs {
  brightness?: boolean;
}

/** `surface.attach` params (host → core, ntf). */
model SurfaceAttach {
  surface: string;
  `model`: string;
  serial?: string;
  name: string;
  controls: ControlDef[];
  outputs?: SurfaceOutputs;
}

/** `surface.detach` params (host → core, ntf). */
model SurfaceDetach {
  surface: string;
}

union InputKind {
  "down",
  "up",
  "turn",
  "touch",
  "value",
}

/** `surface.input` params (host → core, ntf). */
model SurfaceInput {
  surface: string;
  control: string;
  kind: InputKind;

  /** Velocity for `down`, detents for `turn`, 0 to 1 (or `[x, y]`) for `value`, 1 or 0 for `touch`. */
  value?: float64 | float64[];

  /** The host's timestamp, in milliseconds on its own clock. */
  ts: float64;

  echo?: boolean;
}

/** (M7) `rgb`: raw 8-bit RGB, rows top to bottom, for devices whose driver packs images itself. */
union ImageFormat {
  "jpeg",
  "bmp",
  "png",
  "rgb565",
  "rgb",
}

/** `surface.draw` params (core → host, ntf), for `image` outputs. */
model SurfaceDraw {
  surface: string;
  control: string;
  image: Bytes;
  format: ImageFormat;
}
