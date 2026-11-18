/**
 * A simulated show for the server's dataflow acceptance test: a switcher's
 * program input, a mixer's volume, and a GPIO contact. Simulate mode only.
 *
 * State: `program` (an input number), `volume` (0 to 1).
 * Events: `contact`, when the contact closes.
 *
 * @module
 */

import { ActionError, definePlugin, type Plugin, s } from "@covalence-io/sdk";

const showSim: Plugin = definePlugin({
  id: "io.covalence.test.showsim",
  name: "Simulated show",
  version: "0.1.0",
  description: "A switcher, a mixer and a GPIO contact, simulated, for tests.",
  authors: ["Covalence IO contributors"],
  categories: ["test"],
  connections: "multiple",

  config: s.object({}),

  state: {
    program: s.integer({ label: "Program", minimum: 0 }),
    volume: s.number({ label: "Volume", minimum: 0, maximum: 1 }),
  },

  events: {
    contact: s.object({}, { label: "Contact closed" }),
  },

  connect() {
    throw new ActionError("action_failed", "this plugin only simulates");
  },

  async disconnect() {},

  actions: {
    cut: {
      name: "Cut",
      description: "Puts an input on program.",
      params: s.object({ input: s.integer({ label: "Input", minimum: 0 }) }),
      idempotent: true,
      durationHint: "instant",
      run() {
        throw new ActionError("action_failed", "this plugin only simulates");
      },
    },
    setVolume: {
      name: "Set volume",
      description: "Sets the mixer's volume.",
      params: s.object({
        level: s.number({ label: "Level", minimum: 0, maximum: 1 }),
      }),
      idempotent: true,
      durationHint: "instant",
      run() {
        throw new ActionError("action_failed", "this plugin only simulates");
      },
    },
    close: {
      name: "Close the contact",
      description: "As if the GPIO contact closed.",
      params: s.object({}),
      durationHint: "instant",
      run() {
        throw new ActionError("action_failed", "this plugin only simulates");
      },
    },
  },

  simulator: {
    init(sim) {
      sim.state.set({ program: 1, volume: 0.5 });
    },
    actions: {
      cut(sim, { input }) {
        sim.state.patch("program", input);
      },
      setVolume(sim, { level }) {
        sim.state.patch("volume", level);
      },
      close(sim) {
        sim.emit("contact", {});
      },
    },
  },
});

export default showSim;
