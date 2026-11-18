/**
 * Elgato Stream Deck plugin: a deck, through the core (plugin protocol spec §13.6).
 *
 * - The core finds the deck (`devices`), opens it, and makes a connection
 *   for it without asking. This plugin opens it through `ctx.hid` and drives
 *   it with `@elgato-stream-deck/core`, which has no native code.
 * - The deck is a surface of its keys, `k0` … left to right, top to bottom.
 *   Key reports become `surface.input` with the time the core read them;
 *   the core renders each key in the deck's own format and turn, and this
 *   plugin sends the image.
 * - Models with only keys are supported. The MK.2 is the one checked by
 *   hand. Dials, touch strips and LCD strips are out of scope (M7).
 * - Unplugged, the surface detaches; plugged back in, it attaches again and
 *   the core redraws it.
 *
 * State: `model`, `serial`, `firmware`.
 *
 * @module
 */

import {
  DEVICE_MODEL_INFO,
  findModelByUsb,
  getDriver,
  type StreamDeck,
  type StreamDeckButtonControlDefinition,
  type StreamDeckModelInfo,
} from "@elgato-stream-deck/core";
import {
  type BaseContext,
  definePlugin,
  type HidDevice,
  type ImageSpec,
  type Plugin,
  presets,
  s,
  type SurfaceHandle,
} from "@covalence-io/sdk";
import { CoreHidDevice } from "./hid.ts";
import { type JpegHook, probe } from "./images.ts";

export type Config = { device: string };

/** The models this plugin drives: those with keys and nothing else. */
export const KEY_MODELS: StreamDeckModelInfo[] = Object.values(
  DEVICE_MODEL_INFO,
).filter((m) =>
  m.usb.length > 0 && m.controls.length > 0 &&
  m.controls.every((c) => c.type === "button")
);

/** Their USB IDs, grouped by vendor, for the manifest's `devices`. */
function matches() {
  const byVendor = new Map<number, number[]>();
  for (const m of KEY_MODELS) {
    for (const usb of m.usb) {
      const ids = byVendor.get(usb.vendorId) ?? [];
      if (!ids.includes(usb.productId)) ids.push(usb.productId);
      byVendor.set(usb.vendorId, ids);
    }
  }
  return [...byVendor].map(([vendorId, productId]) => ({
    kind: "hid" as const,
    vendorId,
    productId: productId.sort((a, b) => a - b),
    autoCreate: true,
  }));
}

/** One open deck: the library's handle on it, and its surface. */
export class DeckSession {
  readonly #ctx: BaseContext<Config>;
  readonly #device: HidDevice;
  #deck: StreamDeck | undefined;
  #adapter: CoreHidDevice | undefined;
  #hook: JpegHook | undefined;
  #surface: SurfaceHandle | undefined;
  #specs = new Map<number, ImageSpec>();
  /** Keys' images go out one at a time: `encodeJPEG` answers the one at hand. */
  #drawing: Promise<void> = Promise.resolve();
  readonly #blank = new Map<number, Uint8Array>();
  #model: StreamDeckModelInfo | undefined;
  #serial: string | undefined;

  constructor(ctx: BaseContext<Config>, device: HidDevice) {
    this.#ctx = ctx;
    this.#device = device;
  }

  get deck(): StreamDeck | undefined {
    return this.#deck;
  }

  /** Opens the deck if it's there, and follows it being unplugged. */
  async start() {
    this.#device.onState((state, error) => {
      if (state === "open") {
        this.#up().catch((e) => this.#fail(e));
      } else {
        this.#surface?.detach();
        this.#surface = undefined;
        this.#ctx.status("disconnected", error ?? state);
      }
    });
    if (this.#device.state === "open") await this.#up();
    else {this.#ctx.status(
        "disconnected",
        this.#device.error ?? "not plugged in",
      );}
  }

  #fail(e: unknown) {
    this.#ctx.status("error", e instanceof Error ? e.message : String(e));
  }

  /** The deck is (back): open it with the library, the first time, and attach. */
  async #up() {
    if (this.#deck === undefined) await this.#open();
    const model = this.#model!;
    const controls = model.controls
      .filter((c): c is StreamDeckButtonControlDefinition =>
        c.type === "button"
      )
      .map((c) =>
        presets.button({
          id: `k${c.index}`,
          at: { x: c.column, y: c.row },
          image: this.#specs.get(c.index),
        })
      );
    this.#surface?.detach();
    const surface = this.#ctx.surfaces.attach({
      model: `elgato.${model.id}`,
      serial: this.#serial,
      name: model.name,
      controls,
    });
    surface.onDraw(({ control, image, format }) => {
      const index = Number(control.slice(1));
      this.#drawing = this.#drawing
        .then(() => this.#draw(index, image, format))
        .catch((e) => this.#ctx.log.warn(`cannot draw ${control}: ${e}`));
    });
    this.#surface = surface;
    this.#ctx.status("ok");
  }

  async #open() {
    const list = await this.#ctx.devices.list("hid");
    const info = list.find((d) => d.id === this.#ctx.config.device);
    if (!info?.vendorId || !info.productId) {
      throw new Error(`${this.#ctx.config.device} isn't plugged in`);
    }
    const model = findModelByUsb(info.vendorId, info.productId, info.usage);
    const driver = model && getDriver(model.id);
    if (!model || !driver || !KEY_MODELS.includes(model)) {
      throw new Error(
        `${info.product ?? info.id} isn't a Stream Deck this plugin drives`,
      );
    }
    this.#adapter = new CoreHidDevice(this.#device, {
      path: info.path,
      vendorId: info.vendorId,
      productId: info.productId,
    });
    const deck = await driver(this.#adapter, {
      encodeJPEG: (rgba, w, h) => {
        if (!this.#hook) {
          return Promise.reject(new Error("no image to send"));
        }
        return Promise.resolve(this.#hook(rgba, w, h));
      },
    });
    this.#deck = deck;
    this.#model = model;
    for (const c of model.controls) {
      if (c.type !== "button" || c.feedbackType !== "lcd") continue;
      const { width, height } = c.pixelSize;
      this.#specs.set(
        c.index,
        await probe(deck, c.index, width, height, (h) => this.#hook = h),
      );
    }
    this.#serial = await deck.getSerialNumber().catch(() =>
      info.serial ?? undefined
    );
    const firmware = await deck.getFirmwareVersion().catch(() => undefined);
    this.#ctx.state.set({
      model: model.name,
      serial: this.#serial ?? "",
      firmware: firmware ?? "",
    });

    const at = () => this.#adapter?.lastInputTs;
    deck.on("down", (c) => {
      if (c.type === "button") {
        this.#surface?.down(`k${c.index}`, undefined, at());
      }
    });
    deck.on("up", (c) => {
      if (c.type === "button") this.#surface?.up(`k${c.index}`, at());
    });
    deck.on("error", (e) => this.#ctx.log.warn(`deck error: ${e}`));
  }

  async #draw(index: number, image: Uint8Array, format: string) {
    const deck = this.#deck;
    const spec = this.#specs.get(index);
    if (!deck || !spec) return;
    if (format === "rgb") {
      await deck.fillKeyBuffer(index, image, { format: "rgb" });
      return;
    }
    // The library flips a raw image first; it's thrown away, and the
    // core's JPEG goes out as it is.
    let blank = this.#blank.get(spec.w * spec.h);
    if (!blank) {
      blank = new Uint8Array(spec.w * spec.h * 3);
      this.#blank.set(spec.w * spec.h, blank);
    }
    this.#hook = () => image;
    try {
      await deck.fillKeyBuffer(index, blank, { format: "rgb" });
    } finally {
      this.#hook = undefined;
    }
  }

  async setBrightness(percent: number) {
    if (!this.#deck) throw new Error("the deck isn't plugged in");
    await this.#deck.setBrightness(percent);
  }

  stop() {
    this.#surface?.detach();
  }
}

const streamdeck: Plugin = definePlugin<
  Config,
  Record<string, never>,
  DeckSession,
  { brightness: { percent: number } }
>({
  id: "io.covalence.streamdeck",
  name: "Elgato Stream Deck",
  version: "0.1.0",
  description:
    "Stream Decks with keys: the MK.2, XL, Mini, original, modules and others.",
  authors: ["Covalence IO contributors"],
  categories: ["surface"],
  connections: "multiple",

  config: s.object({
    device: s.string({ label: "Device", format: "device", minLength: 1 }),
  }),
  devices: matches(),

  state: {
    model: s.string({ label: "Model", readOnly: true }),
    serial: s.string({ label: "Serial number", readOnly: true }),
    firmware: s.string({ label: "Firmware", readOnly: true }),
  },

  async connect(ctx) {
    const device = await ctx.hid.open(ctx.config.device);
    const session = new DeckSession(ctx, device);
    await session.start();
    return session;
  },

  disconnect(ctx) {
    ctx.session.stop();
  },

  actions: {
    brightness: {
      name: "Set brightness",
      params: s.object({
        percent: s.integer({ label: "Brightness", minimum: 0, maximum: 100 }),
      }),
      run: (ctx, { percent }) => ctx.session.setBrightness(percent),
    },
  },

  simulator: {
    init(sim) {
      sim.state.set({
        model: "Simulated Stream Deck",
        serial: "",
        firmware: "",
      });
    },
    actions: {
      brightness: () => {},
    },
  },
});

export default streamdeck;
