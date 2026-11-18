/**
 * Key images: the core renders each key in the deck's native format, and the
 * library sends it (plugin protocol §8).
 *
 * - **JPEG models** (gen 2): the library flips a raw image and hands it to
 *   `encodeJPEG`. Encoding in JS is far over budget, so the core renders
 *   JPEG, already turned the way the model needs, and `encodeJPEG` hands that
 *   back. What the turn is comes from a probe: the library prepares (and
 *   doesn't send) an image whose pixels hold their own coordinates, and where
 *   they land says which way it turns.
 * - **BMP models** (gen 1): the library packs raw RGB itself, so the core
 *   sends `rgb`.
 *
 * @module
 */

import type { StreamDeck } from "@elgato-stream-deck/core";
import type { ImageSpec } from "@covalence-io/sdk";

type Rotation = 0 | 90 | 180 | 270;

/** What `encodeJPEG` does. The session points it at a probe or an image. */
export type JpegHook = (
  rgba: Uint8Array,
  width: number,
  height: number,
) => Uint8Array;

/**
 * Where the pixel at `(x, y)` of an image the core turned by `rotation`
 * came from, in the upright `w` × `h` image. The inverse of the core's
 * turn (`render.rs`).
 */
export function source(
  rotation: Rotation,
  x: number,
  y: number,
  w: number,
  h: number,
): [number, number] {
  switch (rotation) {
    case 0:
      return [x, y];
    case 90:
      return [y, w - 1 - x];
    case 180:
      return [w - 1 - x, h - 1 - y];
    case 270:
      return [h - 1 - y, x];
  }
}

/** The core's turn that does what the library did to the probe, if any. */
export function rotationOf(
  rgba: Uint8Array,
  w: number,
  h: number,
): Rotation | undefined {
  const points: [number, number][] = [
    [0, 0],
    [w - 1, 0],
    [0, h - 1],
    [w - 1, h - 1],
    [Math.floor(w / 3), Math.floor(h / 5)],
  ];
  return ([0, 90, 180, 270] as const).find((r) =>
    points.every(([x, y]) => {
      const [sx, sy] = source(r, x, y, w, h);
      const at = (y * w + x) * 4;
      return rgba[at] === sx && rgba[at + 1] === sy;
    })
  );
}

/**
 * The image output of key `index`: its size, and the format and turn the
 * core should render it in. `hook` must be the deck's `encodeJPEG`.
 */
export async function probe(
  deck: StreamDeck,
  index: number,
  w: number,
  h: number,
  setHook: (hook: JpegHook | undefined) => void,
): Promise<ImageSpec> {
  if (w > 256 || h > 256) {
    throw new Error(`a ${w}×${h} key is too large to probe`);
  }
  const probeImage = new Uint8Array(w * h * 3);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      probeImage[(y * w + x) * 3] = x;
      probeImage[(y * w + x) * 3 + 1] = y;
    }
  }
  let seen: Uint8Array | undefined;
  setHook((rgba) => {
    seen = rgba;
    return new Uint8Array();
  });
  try {
    await deck.prepareFillKeyBuffer(index, probeImage, { format: "rgb" });
  } finally {
    setHook(undefined);
  }
  if (seen === undefined) return { w, h, format: "rgb" };
  const rotation = rotationOf(seen, w, h);
  if (rotation === undefined) {
    throw new Error("this model turns key images in a way the core can't");
  }
  return { w, h, format: "jpeg", rotation };
}
