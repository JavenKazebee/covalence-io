/**
 * A plugin that opens its own sockets (`directNetwork`, ADR 0025), for
 * checking that its connections are marked as not inspectable. It connects to
 * nothing.
 */

import { definePlugin, type Plugin, s } from "@covalence-io/sdk";

const plugin: Plugin = definePlugin({
  id: "io.covalence.test.direct",
  name: "Direct network test plugin",
  version: "0.0.1",
  directNetwork: true,
  config: s.object({}),
  state: {},
  connect() {
    return Promise.resolve({});
  },
  actions: {},
  simulator: {},
});

export default plugin;
