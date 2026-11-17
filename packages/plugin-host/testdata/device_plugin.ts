/**
 * A plugin for device tests: it declares the devices it drives, and names its
 * device in its config. It does nothing with the device yet.
 */

import { definePlugin, type Plugin, s } from "@covalence-io/sdk";

const plugin: Plugin = definePlugin({
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
  state: {},
  connect: () => undefined,
  actions: {},
  simulator: {},
});

export default plugin;
