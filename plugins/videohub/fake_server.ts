/**
 * A protocol-level fake Videohub (ADR 0012): a TCP server that speaks the
 * Videohub Ethernet protocol, so the real plugin code can be tested without
 * hardware.
 *
 * ```ts
 * const hub = new FakeVideohub({ inputs: 4, outputs: 4 });
 * // connect the plugin to 127.0.0.1:${hub.port} …
 * hub.route(2, 1); // a change made on the front panel
 * await hub.close();
 * ```
 *
 * @module
 */

import {
  serveTcp,
  type TcpClient,
  type TcpSimulator,
} from "@covalence-io/sdk/simulator";
import {
  BlockDecoder,
  formatBlock,
  parseRoutes,
  PRELUDE_END,
} from "./protocol.ts";

export interface FakeVideohubOptions {
  inputs?: number;
  outputs?: number;
  model?: string;
  hostname?: string;
  port?: number;
}

export class FakeVideohub {
  readonly model: string;
  readonly inputLabels: string[];
  readonly outputLabels: string[];
  /** `routes[output]` is the input routed to it. */
  readonly routes: number[];
  /** Every command block received, in order, as text. */
  readonly received: string[] = [];
  readonly #server: TcpSimulator;

  constructor(options: FakeVideohubOptions = {}) {
    const inputs = options.inputs ?? 12;
    const outputs = options.outputs ?? 12;
    this.model = options.model ??
      `Blackmagic Smart Videohub ${inputs}x${outputs}`;
    this.inputLabels = Array.from(
      { length: inputs },
      (_, i) => `Input ${i + 1}`,
    );
    this.outputLabels = Array.from(
      { length: outputs },
      (_, o) => `Output ${o + 1}`,
    );
    this.routes = Array.from({ length: outputs }, (_, o) => o % inputs);
    this.#server = serveTcp((client) => this.#serve(client), {
      hostname: options.hostname,
      port: options.port,
    });
  }

  get port(): number {
    return this.#server.port;
  }

  get hostname(): string {
    return this.#server.hostname;
  }

  /** Number of connected clients. */
  get clients(): number {
    return this.#server.clients.size;
  }

  /** Changes a route as the front panel would, notifying every client. */
  async route(output: number, input: number) {
    this.routes[output] = input;
    await this.#server.broadcast(
      formatBlock("VIDEO OUTPUT ROUTING", [`${output} ${input}`]),
    );
  }

  /** Drops every client, as a reboot would. */
  dropClients() {
    this.#server.dropClients();
  }

  close(): Promise<void> {
    return this.#server.close();
  }

  /** The prelude a device sends on connect. */
  prelude(): string {
    const n = (labels: string[]) => labels.map((l, i) => `${i} ${l}`);
    return [
      formatBlock("PROTOCOL PREAMBLE", ["Version: 2.8"]),
      formatBlock("VIDEOHUB DEVICE", [
        "Device present: true",
        `Model name: ${this.model}`,
        `Video inputs: ${this.inputLabels.length}`,
        "Video processing units: 0",
        `Video outputs: ${this.outputLabels.length}`,
        "Video monitoring outputs: 0",
        "Serial ports: 0",
      ]),
      formatBlock("INPUT LABELS", n(this.inputLabels)),
      formatBlock("OUTPUT LABELS", n(this.outputLabels)),
      formatBlock("VIDEO OUTPUT LOCKS", this.routes.map((_, o) => `${o} U`)),
      formatBlock(
        "VIDEO OUTPUT ROUTING",
        this.routes.map((i, o) => `${o} ${i}`),
      ),
      formatBlock(PRELUDE_END),
    ].join("");
  }

  async #serve(client: TcpClient) {
    await client.write(this.prelude());
    const decoder = new BlockDecoder();
    for await (const chunk of client.chunks()) {
      for (const block of decoder.push(chunk)) {
        this.received.push(formatBlock(block.header, block.lines));
        switch (block.header) {
          case "PING":
            await client.write(formatBlock("ACK"));
            break;
          case "VIDEO OUTPUT ROUTING": {
            const routes = parseRoutes(block.lines);
            const valid = routes.size === block.lines.length &&
              [...routes].every(([o, i]) =>
                o < this.routes.length && i < this.inputLabels.length
              );
            if (!valid) {
              await client.write(formatBlock("NAK"));
              break;
            }
            await client.write(formatBlock("ACK"));
            const lines: string[] = [];
            for (const [o, i] of routes) {
              this.routes[o] = i;
              lines.push(`${o} ${i}`);
            }
            // The device echoes accepted changes to every client.
            await this.#server.broadcast(
              formatBlock("VIDEO OUTPUT ROUTING", lines),
            );
            break;
          }
          default:
            await client.write(formatBlock("NAK"));
        }
      }
    }
  }
}
