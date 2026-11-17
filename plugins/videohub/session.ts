/**
 * Live mode: a TCP session with a Videohub, through the core's transport
 * (`ctx.tcp`, plugin protocol §7), so the core reconnects it and the
 * connection's traffic can be inspected.
 *
 * - Opens one socket with `reconnect`; the core retries with backoff, and the
 *   socket's state becomes `conn.status`.
 * - Builds the state from the device's prelude on each link and publishes it
 *   when `END PRELUDE` arrives; after that, routing blocks become patches and
 *   `routeChanged` events.
 * - Sends commands and waits for the device's `ACK` / `NAK`.
 * - A link that sends no prelude is closed and opened afresh.
 *
 * @module
 */

import { ActionError, type BaseContext, type TcpConn } from "@covalence-io/sdk";
import type { ConnStatusKind, IoStateKind } from "@covalence-io/protocol";
import {
  type Block,
  BlockDecoder,
  formatBlock,
  parseFields,
  parseLabels,
  parseRoutes,
  PRELUDE_END,
} from "./protocol.ts";

export interface Config {
  host: string;
  port: number;
}

export interface Events {
  routeChanged: { output: number; input: number };
}

export type VideohubContext = BaseContext<Config, Events>;

export interface SessionOptions {
  /** How long to wait for the prelude after connecting. */
  preludeTimeoutMs?: number;
  /** How long to wait for `ACK` / `NAK`. */
  replyTimeoutMs?: number;
  /** First reconnect delay; doubles up to `maxRetryMs`. */
  retryMs?: number;
  maxRetryMs?: number;
}

interface Waiter {
  resolve(): void;
  reject(err: Error): void;
  timer: ReturnType<typeof setTimeout>;
}

interface Staging {
  model?: string;
  inputs: Record<number, { name: string }>;
  outputs: Record<number, { name?: string; source?: number }>;
}

const encoder = new TextEncoder();

export class VideohubSession {
  readonly #ctx: VideohubContext;
  readonly #opts: Required<SessionOptions>;
  #conn: TcpConn | undefined;
  /** The socket is open (the prelude may not have arrived yet). */
  #open = false;
  #ready = false;
  #decoder = new BlockDecoder();
  #staging: Staging | undefined;
  #preludeTimer: ReturnType<typeof setTimeout> | undefined;
  #noPrelude = false;
  /** The wait before a fresh socket; reset by a prelude. */
  #delay = 0;
  #waiters: Waiter[] = [];
  #status: ConnStatusKind = "connecting";
  #started: (() => void) | undefined;
  #loop: Promise<void> | undefined;
  /** Device size, from the prelude. */
  inputs = 0;
  outputs = 0;

  constructor(ctx: VideohubContext, options: SessionOptions = {}) {
    this.#ctx = ctx;
    this.#opts = {
      preludeTimeoutMs: options.preludeTimeoutMs ?? 5000,
      replyTimeoutMs: options.replyTimeoutMs ?? 2000,
      retryMs: options.retryMs ?? 500,
      maxRetryMs: options.maxRetryMs ?? 5000,
    };
  }

  /** True once the prelude has arrived on the current link. */
  get connected(): boolean {
    return this.#ready;
  }

  /**
   * Starts the link. Resolves after the first prelude, or after the first
   * attempt fails (the status is then `error` and retries continue).
   */
  start(): Promise<void> {
    return new Promise((resolve) => {
      this.#started = resolve;
      this.#loop = this.#run();
    });
  }

  /** Closes the link and stops reconnecting. `ctx.signal` must be aborted. */
  async close() {
    this.#conn?.close();
    await this.#loop;
  }

  /** Routes `input` to `output` and waits for the device to accept it. */
  async route(output: number, input: number) {
    if (!this.#ready) throw new ActionError("not_connected", "not connected");
    if (output >= this.outputs) {
      throw new ActionError("invalid_params", `no output ${output}`);
    }
    if (input >= this.inputs) {
      throw new ActionError("invalid_params", `no input ${input}`);
    }
    await this.#command(
      formatBlock("VIDEO OUTPUT ROUTING", [`${output} ${input}`]),
    );
  }

  #command(block: string): Promise<void> {
    const conn = this.#conn;
    if (!conn || !this.#open) {
      return Promise.reject(new ActionError("not_connected", "not connected"));
    }
    const reply = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        const i = this.#waiters.indexOf(waiter);
        if (i >= 0) this.#waiters.splice(i, 1);
        reject(new ActionError("timeout", "no reply from the Videohub"));
      }, this.#opts.replyTimeoutMs);
      const waiter: Waiter = { resolve, reject, timer };
      this.#waiters.push(waiter);
    });
    // The core writes in order; a write lost to a dropped link times out.
    conn.write(encoder.encode(block));
    return reply;
  }

  #begin() {
    const started = this.#started;
    this.#started = undefined;
    started?.();
  }

  #setStatus(status: ConnStatusKind, message?: string) {
    if (status === this.#status) return;
    this.#status = status;
    this.#ctx.status(status, message);
  }

  /** The link is down: report it, unless it already was. */
  #down(error: string) {
    const { host, port } = this.#ctx.config;
    if (this.#started) {
      this.#ctx.log.warn(`videohub link down: ${error}`, { host, port });
      this.#setStatus("error", error);
      this.#begin();
    } else if (this.#status === "ok") {
      this.#ctx.log.warn(`videohub link down: ${error}`, { host, port });
      this.#setStatus("disconnected", error);
    }
  }

  async #run() {
    const { host, port } = this.#ctx.config;
    const signal = this.#ctx.signal;
    this.#delay = this.#opts.retryMs;
    while (!signal.aborted) {
      let conn: TcpConn;
      try {
        conn = await this.#ctx.tcp.connect({
          hostname: host,
          port,
          reconnect: true,
          backoff: {
            initialMs: this.#opts.retryMs,
            maxMs: this.#opts.maxRetryMs,
          },
          framing: "line",
        });
      } catch (err) {
        // Not a refusal (the core retries those): a bad target, or no core.
        this.#down(err instanceof Error ? err.message : String(err));
        break;
      }
      if (signal.aborted) {
        conn.close();
        break;
      }
      this.#conn = conn;
      this.#noPrelude = false;
      conn.onState((state, error) => this.#onState(state, error));
      const buf = new Uint8Array(16 * 1024);
      for (;;) {
        const n = await conn.read(buf);
        if (n === null) break;
        if (!this.#open) continue;
        for (const block of this.#decoder.push(buf.subarray(0, n))) {
          if (this.#handle(block)) this.#prelude();
        }
      }
      this.#conn = undefined;
      this.#dropLink();
      if (signal.aborted) break;
      // No prelude, or the core gave up on the socket: a fresh one, later.
      this.#down(
        this.#noPrelude ? "no prelude from the Videohub" : "connection closed",
      );
      await sleep(this.#delay, signal);
      this.#delay = Math.min(this.#delay * 2, this.#opts.maxRetryMs);
    }
    this.#begin();
  }

  #onState(state: IoStateKind, error: string | undefined) {
    if (state === "open") {
      this.#open = true;
      this.#decoder = new BlockDecoder();
      this.#staging = { inputs: {}, outputs: {} };
      clearTimeout(this.#preludeTimer);
      this.#preludeTimer = setTimeout(() => {
        this.#noPrelude = true;
        this.#conn?.close();
      }, this.#opts.preludeTimeoutMs);
      return;
    }
    this.#dropLink();
    if (state !== "closed") this.#down(error ?? state);
  }

  #prelude() {
    clearTimeout(this.#preludeTimer);
    this.#delay = this.#opts.retryMs;
    if (this.#started) {
      this.#status = "ok"; // the runtime reports ok for the first prelude
      this.#begin();
    } else {
      this.#setStatus("ok");
    }
  }

  // Returns true when this block completes the prelude.
  #handle(block: Block): boolean {
    const staging = this.#staging;
    switch (block.header) {
      case "ACK":
      case "NAK": {
        const waiter = this.#waiters.shift();
        if (!waiter) return false;
        clearTimeout(waiter.timer);
        if (block.header === "ACK") waiter.resolve();
        else {
          waiter.reject(
            new ActionError(
              "action_failed",
              "the Videohub refused the command",
            ),
          );
        }
        return false;
      }
      case "VIDEOHUB DEVICE": {
        const fields = parseFields(block.lines);
        const inputs = Number(fields.get("Video inputs"));
        const outputs = Number(fields.get("Video outputs"));
        if (Number.isInteger(inputs)) this.inputs = inputs;
        if (Number.isInteger(outputs)) this.outputs = outputs;
        const model = fields.get("Model name");
        if (model !== undefined) {
          if (staging) staging.model = model;
          else this.#ctx.state.patch("model", model);
        }
        return false;
      }
      case "INPUT LABELS":
        for (const [i, name] of parseLabels(block.lines)) {
          if (staging) staging.inputs[i] = { name };
          else this.#ctx.state.patch(["inputs", i, "name"], name);
        }
        return false;
      case "OUTPUT LABELS":
        for (const [o, name] of parseLabels(block.lines)) {
          if (staging) (staging.outputs[o] ??= {}).name = name;
          else this.#ctx.state.patch(["outputs", o, "name"], name);
        }
        return false;
      case "VIDEO OUTPUT ROUTING":
        for (const [o, i] of parseRoutes(block.lines)) {
          if (staging) {
            (staging.outputs[o] ??= {}).source = i;
          } else if (this.#ctx.state.get(["outputs", o, "source"]) !== i) {
            this.#ctx.state.patch(["outputs", o, "source"], i);
            this.#ctx.emit("routeChanged", { output: o, input: i });
          }
        }
        return false;
      case PRELUDE_END:
        if (!staging) return false;
        this.#ctx.state.set({
          model: staging.model ?? "Videohub",
          inputs: staging.inputs,
          outputs: staging.outputs,
        });
        this.#staging = undefined;
        this.#ready = true;
        return true;
      default:
        return false; // PROTOCOL PREAMBLE, locks, serial ports, …
    }
  }

  #dropLink() {
    clearTimeout(this.#preludeTimer);
    this.#open = false;
    this.#ready = false;
    this.#staging = undefined;
    for (const w of this.#waiters.splice(0)) {
      clearTimeout(w.timer);
      w.reject(new ActionError("not_connected", "the Videohub link dropped"));
    }
  }
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve();
    const timer = setTimeout(done, ms);
    signal.addEventListener("abort", done, { once: true });
    function done() {
      clearTimeout(timer);
      signal.removeEventListener("abort", done);
      resolve();
    }
  });
}
