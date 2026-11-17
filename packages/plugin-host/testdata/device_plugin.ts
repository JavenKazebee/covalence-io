/**
 * A plugin for device and transport tests. It declares the devices it drives
 * and names its device in its config. Live, it opens that device through
 * `ctx.hid` (or `ctx.serial`) and keeps the last input in state; its actions
 * write to the device, read feature reports, list devices and talk TCP.
 */

import {
  definePlugin,
  type HidDevice,
  type IoHandle,
  type Plugin,
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
    // A Stream Deck MK.2: created without asking.
    { kind: "hid", vendorId: 0x0fd9, productId: [0x0080], autoCreate: true },
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
