/**
 * Live mode: a TCP session with a Videohub, using Deno sockets directly (host
 * transports are deferred, M1).
 *
 * - Holds the link open and reconnects with backoff, reporting `conn.status`.
 * - Builds the state from the device's prelude and publishes it when
 *   `END PRELUDE` arrives; after that, routing blocks become patches and
 *   `routeChanged` events.
 * - Sends commands and waits for the device's `ACK` / `NAK`.
 *
 * @module
 */

import { ActionError, type BaseContext } from "@covalence-io/sdk";
import type { ConnStatusKind } from "@covalence-io/protocol";
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
  #conn: Deno.TcpConn | undefined;
  #ready = false;
  #staging: Staging | undefined;
  #waiters: Waiter[] = [];
  #writes: Promise<void> = Promise.resolve();
  #status: ConnStatusKind = "connecting";
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
      this.#loop = this.#run(resolve);
    });
  }

  /** Closes the link and stops reconnecting. `ctx.signal` must be aborted. */
  async close() {
    if (this.#conn) closeQuietly(this.#conn);
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
    if (!conn) {
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
    const bytes = encoder.encode(block);
    this.#writes = this.#writes.then(async () => {
      let off = 0;
      while (off < bytes.length) off += await conn.write(bytes.subarray(off));
    }).catch(() => {}); // a write error closes the link; the read loop sees it
    return reply;
  }

  #setStatus(status: ConnStatusKind, message?: string) {
    if (status === this.#status) return;
    this.#status = status;
    this.#ctx.status(status, message);
  }

  async #run(started: () => void) {
    const { host, port } = this.#ctx.config;
    const signal = this.#ctx.signal;
    let first = true;
    let delay = this.#opts.retryMs;
    while (!signal.aborted) {
      let error: string | undefined;
      try {
        const conn = await Deno.connect({ hostname: host, port });
        if (signal.aborted) {
          conn.close();
          break;
        }
        this.#conn = conn;
        delay = this.#opts.retryMs;
        error = await this.#read(conn, () => {
          if (first) {
            first = false;
            this.#status = "ok"; // the runtime reports ok for the first prelude
            started();
          } else {
            this.#setStatus("ok");
          }
        });
      } catch (err) {
        error = err instanceof Error ? err.message : String(err);
      }
      this.#dropLink();
      if (signal.aborted) break;
      this.#ctx.log.warn(`videohub link down: ${error}`, { host, port });
      if (first) {
        first = false;
        this.#setStatus("error", error);
        started();
      } else {
        this.#setStatus(
          this.#status === "ok" ? "disconnected" : this.#status,
          error,
        );
      }
      await sleep(delay, signal);
      delay = Math.min(delay * 2, this.#opts.maxRetryMs);
    }
    started();
  }

  // Reads until the link closes; returns why it closed.
  async #read(conn: Deno.TcpConn, onReady: () => void): Promise<string> {
    const decoder = new BlockDecoder();
    this.#staging = { inputs: {}, outputs: {} };
    let timedOut = false;
    const preludeTimer = setTimeout(() => {
      timedOut = true;
      closeQuietly(conn);
    }, this.#opts.preludeTimeoutMs);
    const buf = new Uint8Array(16 * 1024);
    try {
      for (;;) {
        const n = await conn.read(buf);
        if (n === null) return "connection closed by the Videohub";
        for (const block of decoder.push(buf.subarray(0, n))) {
          if (this.#handle(block)) {
            clearTimeout(preludeTimer);
            onReady();
          }
        }
      }
    } catch (err) {
      if (timedOut) return "no prelude from the Videohub";
      throw err;
    } finally {
      clearTimeout(preludeTimer);
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
    if (this.#conn) closeQuietly(this.#conn);
    this.#conn = undefined;
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

function closeQuietly(conn: Deno.Conn) {
  try {
    conn.close();
  } catch {
    // already closed
  }
}
