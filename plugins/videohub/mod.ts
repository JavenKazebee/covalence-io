/**
 * Blackmagic Videohub plugin.
 *
 * - Live mode talks the Videohub Ethernet protocol over TCP (port 9990).
 * - Simulate mode uses a state-level simulator: a 12×12 router.
 * - `./fake-server` is a protocol-level fake Videohub for tests (ADR 0012).
 *
 * State (paths relative to the connection):
 *
 * - `model`
 * - `inputs[<n>].name`
 * - `outputs[<n>].name`, `outputs[<n>].source`
 *
 * Inputs and outputs are numbered from 0, as on the wire.
 *
 * @module
 */

import {
  ActionError,
  definePlugin,
  type Plugin,
  s,
  type Schema,
} from "@covalence-io/sdk";
import { type Config, type Events, VideohubSession } from "./session.ts";

export type { Config, Events };

/** Size of the simulated router. */
export const SIM_INPUTS = 12;
export const SIM_OUTPUTS = 12;

const index = (from: string, label: string): Schema<number> =>
  s.integer({ minimum: 0, label, choices: { from } });

const videohub: Plugin = definePlugin({
  id: "io.covalence.videohub",
  name: "Blackmagic Videohub",
  version: "0.1.0",
  description: "Video routers from the Blackmagic Videohub family.",
  authors: ["Covalence IO contributors"],
  categories: ["video-routing"],
  connections: "multiple",

  config: s.object({
    host: s.string({ label: "IP address", format: "hostname", minLength: 1 }),
    port: s.integer({
      label: "Port",
      default: 9990,
      minimum: 1,
      maximum: 65535,
    }),
  }),
  permissions: { net: ["{config.host}:{config.port}"] },

  state: {
    model: s.string({ label: "Model", readOnly: true }),
    inputs: s.collection({ key: "index", label: "name" }, {
      name: s.string({ label: "Name" }),
    }),
    outputs: s.collection({ key: "index", label: "name" }, {
      name: s.string({ label: "Name" }),
      source: s.integer({ label: "Source", kind: "id" }),
    }),
  },

  events: {
    routeChanged: s.object({
      output: s.integer({ minimum: 0 }),
      input: s.integer({ minimum: 0 }),
    }, { label: "Route changed" }),
  },

  async connect(ctx) {
    const session = new VideohubSession(ctx);
    await session.start();
    return session;
  },

  async disconnect(ctx) {
    await ctx.session.close();
  },

  actions: {
    route: {
      name: "Set route",
      description: "Routes an input to an output.",
      params: s.object({
        output: index("outputs", "Output"),
        input: index("inputs", "Input"),
      }),
      idempotent: true,
      durationHint: "instant",
      async run(ctx, { output, input }) {
        await ctx.session.route(output, input);
      },
    },
  },

  simulator: {
    init(sim) {
      const inputs: Record<number, { name: string }> = {};
      const outputs: Record<number, { name: string; source: number }> = {};
      for (let i = 0; i < SIM_INPUTS; i++) {
        inputs[i] = { name: `Input ${i + 1}` };
      }
      for (let o = 0; o < SIM_OUTPUTS; o++) {
        outputs[o] = { name: `Output ${o + 1}`, source: o % SIM_INPUTS };
      }
      sim.state.set({ model: "Simulated Videohub 12x12", inputs, outputs });
    },
    actions: {
      route(sim, { output, input }) {
        if (output >= SIM_OUTPUTS) {
          throw new ActionError("invalid_params", `no output ${output}`);
        }
        if (input >= SIM_INPUTS) {
          throw new ActionError("invalid_params", `no input ${input}`);
        }
        if (sim.state.get(["outputs", output, "source"]) === input) return;
        sim.state.patch(`outputs[${output}].source`, input);
        sim.emit("routeChanged", { output, input });
      },
    },
  },
  protocolSimulator: true,
});

export default videohub;
