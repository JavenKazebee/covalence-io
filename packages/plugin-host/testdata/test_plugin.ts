/** A plugin for host tests: slow actions, crashes and console output. */

import { definePlugin, type Plugin, s } from "@covalence-io/sdk";

const plugin: Plugin = definePlugin({
  id: "io.covalence.test",
  name: "Host test plugin",
  version: "0.0.1",
  config: s.object({}),
  state: { n: s.integer() },
  actions: {
    sleep: {
      name: "Sleep",
      params: s.object({ ms: s.integer() }),
      cancellable: true,
      run: () => {},
    },
    crash: {
      name: "Crash the worker",
      params: s.object({}),
      run: () => {},
    },
  },
  simulator: {
    init(sim) {
      console.log("hello from the plugin"); // must not reach stdout
      sim.state.set({ n: 0 });
    },
    actions: {
      sleep: (sim, { ms }, { signal }) =>
        new Promise<void>((resolve, reject) => {
          const timer = setTimeout(() => {
            sim.state.set("n", ms);
            resolve();
          }, ms);
          signal.addEventListener("abort", () => {
            clearTimeout(timer);
            reject(signal.reason);
          });
        }),
      crash: () => {
        setTimeout(() => {
          throw new Error("boom");
        }, 0);
      },
    },
  },
});

export default plugin;
