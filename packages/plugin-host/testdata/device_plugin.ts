/**
 * A plugin for device, transport and surface tests. It declares the devices
 * it drives and names its device in its config. Live, it opens that device
 * through `ctx.hid` (or `ctx.serial`) and keeps the last input in state; its
 * actions write to the device, read feature reports, list devices and talk
 * TCP.
 *
 * A HID device is also a 15-key surface, `test.deck15` with the device ID as
 * its serial. Input report `[0x01, key, pressed]` presses and releases key
 * `k<key>`, and each image the core draws is written to the device as
 * `[0x07, key, ...the image's first 4 bytes]`.
 */

import {
  definePlugin,
  type HidDevice,
  type IoHandle,
  type Plugin,
  presets,
  s,
  type SerialPort,
} from "@covalence-io/sdk";

type Session = { device: HidDevice | SerialPort };

const hex = (data: Uint8Array) =>
  [...data].map((b) => b.toString(16).padStart(2, "0")).join("");
const bytes = (hex: string) =>
  Uint8Array.from(hex.match(/../g) ?? [], (h) => parseInt(h, 16));

const plugin: Plugin = definePlugin<
  { device: string },
  Record<string, never>,
  Session,
  {
    send: { hex: string };
    feature: { reportId: number; length: number };
    sendFeature: { hex: string };
    list: Record<string, never>;
    tcp: { port: number; text: string; via: string };
  }
>({
  id: "io.covalence.test.deck",
  name: "Device test plugin",
  version: "0.0.1",
  config: s.object({
    device: s.string({ label: "Device", format: "device" }),
  }),
  devices: [
    // A test deck (pid.codes' test vendor ID): created without asking.
    { kind: "hid", vendorId: 0x1209, productId: [0x0001], autoCreate: true },
    // An ESP32 over USB CDC: offered.
    { kind: "serial", vendorId: 0x303a },
  ],
  state: {
    input: s.string(),
    io: s.string(),
    inputs: s.integer(),
  },
  async connect(ctx) {
    const device: IoHandle = ctx.config.device.startsWith("serial")
      ? await ctx.serial.open(ctx.config.device, { baud: 9600 })
      : await ctx.hid.open(ctx.config.device);
    let inputs = 0;
    device.onData((data) => {
      ctx.state.set("input", hex(data));
      ctx.state.set("inputs", ++inputs);
    });
    if (device.kind === "hid") {
      const deck = ctx.surfaces.attach({
        model: "test.deck15",
        serial: ctx.config.device,
        name: "Test deck",
        controls: presets.keys(5, 3, {
          w: 72,
          h: 72,
          format: "jpeg",
          rotation: 180,
        }),
      });
      device.onData((data) => {
        if (data[0] !== 0x01) return;
        if (data[2]) deck.down(`k${data[1]}`);
        else deck.up(`k${data[1]}`);
      });
      deck.onDraw(({ control, image }) => {
        const key = Number(control.slice(1));
        device.write(Uint8Array.of(0x07, key, ...image.subarray(0, 4)));
      });
    }
    ctx.state.set("io", device.state);
    device.onState((state, error) =>
      ctx.state.set("io", error ? `${state}:${error}` : state)
    );
    return { device: device as HidDevice | SerialPort };
  },
  actions: {
    send: {
      name: "Write bytes",
      params: s.object({ hex: s.string() }),
      run: (ctx, { hex }) => ctx.session.device.write(bytes(hex)),
    },
    feature: {
      name: "Read a feature report",
      params: s.object({ reportId: s.integer(), length: s.integer() }),
      run: async (ctx, { reportId, length }) =>
        hex(
          await (ctx.session.device as HidDevice).getFeatureReport(
            reportId,
            length,
          ),
        ),
    },
    sendFeature: {
      name: "Send a feature report",
      params: s.object({ hex: s.string() }),
      run: (ctx, { hex }) =>
        (ctx.session.device as HidDevice).sendFeatureReport(bytes(hex)),
    },
    list: {
      name: "List devices",
      params: s.object({}),
      run: async (ctx) => (await ctx.devices.list()).map((d) => d.id),
    },
    tcp: {
      name: "Say something over TCP and return the first reply",
      params: s.object({
        port: s.integer(),
        text: s.string(),
        via: s.string({ description: "conn or net" }),
      }),
      run: async (ctx, { port, text, via }) => {
        if (via === "conn") {
          const conn = await ctx.tcp.connect({ port });
          await conn.write(new TextEncoder().encode(text));
          const buf = new Uint8Array(1024);
          const n = await conn.read(buf);
          conn.close();
          return new TextDecoder().decode(buf.subarray(0, n ?? 0));
        }
        const socket = ctx.net.socket();
        socket.setEncoding("utf-8");
        return await new Promise<string>((resolve, reject) => {
          socket.once("data", (reply: string) => {
            socket.end();
            resolve(reply);
          });
          socket.once("error", reject);
          socket.connect(port, "127.0.0.1", () => socket.write(text));
        });
      },
    },
  },
  simulator: {},
});

export default plugin;
