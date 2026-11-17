import { assert, assertEquals } from "@std/assert";
import {
  DeviceModelId,
  getDriver,
  type HIDDevice,
  type HIDDeviceEvents,
  type StreamDeck,
} from "@elgato-stream-deck/core";
import { EventEmitter } from "eventemitter3";
import plugin, { KEY_MODELS } from "./mod.ts";
import { type JpegHook, probe, rotationOf, source } from "./images.ts";

/** A deck that records what the library sends it. */
class StubDevice extends EventEmitter<HIDDeviceEvents> implements HIDDevice {
  reports: Uint8Array[] = [];
  close = () => Promise.resolve();
  sendFeatureReport = () => Promise.resolve();
  getFeatureReport = (id: number) => Promise.resolve(Uint8Array.of(id, 0));
  sendReports(buffers: Uint8Array[]) {
    this.reports.push(...buffers);
    return Promise.resolve();
  }
  getDeviceInfo = () =>
    Promise.resolve({ path: "x", vendorId: 0x0fd9, productId: 0x0080 });
  getChildDeviceInfo = () => Promise.resolve(null);
}

async function open(id: DeviceModelId) {
  const device = new StubDevice();
  let hook: JpegHook | undefined;
  const deck: StreamDeck = await getDriver(id)!(device, {
    encodeJPEG: (rgba, w, h) => Promise.resolve(hook!(rgba, w, h)),
  });
  return { device, deck, setHook: (h: JpegHook | undefined) => hook = h };
}

Deno.test("the manifest matches keys-only decks and creates their connections", () => {
  const [match] = plugin.manifest.devices!;
  assertEquals(match.vendorId, 0x0fd9);
  assertEquals(match.autoCreate, true);
  const ids = match.productId as number[];
  assert(ids.includes(0x0080), "MK.2");
  assert(ids.includes(0x00a5), "MK.2 with scissor keys");
  assert(!ids.includes(0x0084), "not the Plus: it has dials");
  assert(KEY_MODELS.every((m) => m.controls.every((c) => c.type === "button")));
});

Deno.test("the core's turn is undone exactly", () => {
  const w = 4, h = 4;
  for (const r of [0, 90, 180, 270] as const) {
    // An image whose pixels hold where they came from, turned by r.
    const rgba = new Uint8Array(w * h * 4);
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const [sx, sy] = source(r, x, y, w, h);
        rgba[(y * w + x) * 4] = sx;
        rgba[(y * w + x) * 4 + 1] = sy;
      }
    }
    assertEquals(rotationOf(rgba, w, h), r);
  }
});

Deno.test("an MK.2 takes JPEG turned 180°, an original raw RGB", async () => {
  const mk2 = await open(DeviceModelId.ORIGINALMK2);
  assertEquals(await probe(mk2.deck, 0, 72, 72, mk2.setHook), {
    w: 72,
    h: 72,
    format: "jpeg",
    rotation: 180,
  });
  assertEquals(mk2.device.reports.length, 0, "probing sends nothing");

  const original = await open(DeviceModelId.ORIGINAL);
  assertEquals(await probe(original.deck, 0, 72, 72, original.setHook), {
    w: 72,
    h: 72,
    format: "rgb",
  });
});

Deno.test("the core's JPEG reaches the deck unchanged", async () => {
  const { device, deck, setHook } = await open(DeviceModelId.ORIGINALMK2);
  const jpeg = new Uint8Array(1500).map((_, i) => i % 251);
  setHook(() => jpeg);
  await deck.fillKeyBuffer(3, new Uint8Array(72 * 72 * 3), { format: "rgb" });
  // Gen-2 image reports: [0x02, 0x07, key, last, len (LE), part (LE), …].
  const parts = device.reports.filter((r) => r[0] === 0x02 && r[1] === 0x07);
  assertEquals(parts.map((r) => r[2]), [3, 3]);
  const body = parts.flatMap((r) => [...r.subarray(8, 8 + (r[4] | r[5] << 8))]);
  assertEquals(body, [...jpeg]);
  assertEquals(parts.at(-1)![3], 1, "the last part says so");
});
