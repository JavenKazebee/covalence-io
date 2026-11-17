/**
 * Test helpers for plugins.
 *
 * `LocalTcp` stands in for the core's `tcp` transport (plugin protocol §7)
 * with Deno sockets, so a plugin on `ctx.tcp` can be tested live without the
 * core. It follows `covalence_core::io`: `io.open` answers after the first
 * attempt and fails `not_connected` unless `reconnect` is set; `io.state`
 * comes only after that answer, `open` on each connect and `reconnecting` or
 * `closed` on each loss; writes while away are dropped; framing is the same.
 *
 * @module
 */

import { ActionError } from "./plugin.ts";
import { toBytes } from "./io.ts";
import type { Bytes, Framing, IoOptions } from "@covalence-io/protocol";

const MAX_FRAME = 16 * 1024 * 1024;

/** Where `LocalTcp` sends `io.data` and `io.state`. */
export type Deliver = (method: string, params: unknown) => void;

interface Socket {
  conn: Deno.TcpConn | undefined;
  closed: boolean;
  wake: (() => void) | undefined;
  /** Writes go out in order, as the core's do. */
  writes: Promise<void>;
}

export class LocalTcp {
  readonly #deliver: Deliver;
  readonly #sockets = new Map<string, Socket>();
  #next = 1;

  constructor(deliver: Deliver) {
    this.#deliver = deliver;
  }

  /**
   * Answers `io.open` for `tcp`. Returns `undefined` for anything else, so a
   * caller can chain its own answers.
   */
  request(method: string, params: unknown): Promise<unknown> | undefined {
    if (method !== "io.open") return undefined;
    const p = params as { kind: string; target: string; options?: IoOptions };
    if (p.kind !== "tcp") return undefined;
    return this.#open(p.target, p.options ?? {});
  }

  /** Handles `io.write` and `io.close`; returns whether it was one of them. */
  notify(method: string, params: unknown): boolean {
    const p = params as { handle: string; data?: Bytes };
    const socket = this.#sockets.get(p.handle);
    if (method === "io.write") {
      if (!socket) return false;
      const conn = socket.conn;
      if (!conn) return true; // dropped while away, as the core does
      const bytes = toBytes(p.data ?? []);
      socket.writes = socket.writes.then(() => writeAll(conn, bytes))
        .catch(() => {});
      return true;
    }
    if (method === "io.close") {
      if (!socket) return false;
      this.#close(p.handle, socket);
      return true;
    }
    return false;
  }

  /** Closes every socket. */
  closeAll() {
    for (const [handle, socket] of this.#sockets) this.#close(handle, socket);
  }

  #close(handle: string, socket: Socket) {
    socket.closed = true;
    try {
      socket.conn?.close();
    } catch { /* already closed */ }
    socket.wake?.();
    this.#sockets.delete(handle);
  }

  async #open(target: string, options: IoOptions): Promise<unknown> {
    const i = target.lastIndexOf(":");
    const port = Number(target.slice(i + 1));
    if (i < 0 || !Number.isInteger(port)) {
      throw new ActionError(
        "invalid_params",
        `tcp target ${target} isn't host:port`,
      );
    }
    const hostname = target.slice(0, i);
    const handle = `t${this.#next++}`;
    const socket: Socket = {
      conn: undefined,
      closed: false,
      wake: undefined,
      writes: Promise.resolve(),
    };
    let first: Deno.TcpConn | string;
    try {
      first = await Deno.connect({ hostname, port });
    } catch (err) {
      first = err instanceof Error ? err.message : String(err);
    }
    if (typeof first === "string" && !options.reconnect) {
      throw new ActionError(
        "not_connected",
        `cannot connect to ${target}: ${first}`,
      );
    }
    this.#sockets.set(handle, socket);
    // `io.state` only once `io.open` is answered.
    setTimeout(() => this.#run(handle, socket, hostname, port, options, first));
    return { handle };
  }

  async #run(
    handle: string,
    socket: Socket,
    hostname: string,
    port: number,
    options: IoOptions,
    first: Deno.TcpConn | string,
  ) {
    const initial = options.backoff?.initialMs ?? 500;
    const max = options.backoff?.maxMs ?? 10_000;
    let delay = initial;
    let attempt: Deno.TcpConn | string | undefined = first;
    while (!socket.closed) {
      if (attempt === undefined) {
        try {
          attempt = await Deno.connect({ hostname, port });
        } catch (err) {
          attempt = err instanceof Error ? err.message : String(err);
        }
      }
      let lost: string;
      if (typeof attempt === "string") {
        lost = attempt;
      } else if (socket.closed) {
        attempt.close();
        return;
      } else {
        socket.conn = attempt;
        delay = initial;
        this.#state(handle, "open");
        lost = await this.#serve(handle, attempt, options.framing ?? "raw");
        socket.conn = undefined;
        if (socket.closed) return;
      }
      attempt = undefined;
      if (!options.reconnect) {
        this.#state(handle, "closed", lost);
        this.#sockets.delete(handle);
        return;
      }
      this.#state(handle, "reconnecting", lost);
      await new Promise<void>((resolve) => {
        const timer = setTimeout(done, delay);
        socket.wake = done;
        function done() {
          clearTimeout(timer);
          resolve();
        }
      });
      socket.wake = undefined;
      delay = Math.min(delay * 2, max);
    }
  }

  async #serve(
    handle: string,
    conn: Deno.TcpConn,
    framing: Framing,
  ): Promise<string> {
    const buf = new Uint8Array(64 * 1024);
    let pending: Uint8Array = new Uint8Array(0);
    const data = (d: Uint8Array) =>
      this.#deliver("io.data", { handle, data: d.slice(), ts: Date.now() });
    try {
      for (;;) {
        const n = await conn.read(buf);
        if (n === null) return "closed by the peer";
        const read = buf.subarray(0, n);
        if (framing === "raw") {
          data(read);
          continue;
        }
        pending = concat(pending, read);
        if (framing === "line") {
          let end: number;
          while ((end = pending.indexOf(10)) >= 0) {
            data(pending.subarray(0, end + 1));
            pending = pending.slice(end + 1);
          }
        } else {
          while (pending.length >= 4) {
            const len = new DataView(pending.buffer, pending.byteOffset)
              .getUint32(0);
            if (len > MAX_FRAME) return `a ${len}-byte frame is too large`;
            if (pending.length < 4 + len) break;
            data(pending.subarray(4, 4 + len));
            pending = pending.slice(4 + len);
          }
        }
      }
    } catch (err) {
      return err instanceof Error ? err.message : String(err);
    } finally {
      try {
        conn.close();
      } catch { /* already closed */ }
    }
  }

  #state(handle: string, state: string, error?: string) {
    const params: Record<string, unknown> = { handle, state };
    if (error !== undefined) params.error = error;
    this.#deliver("io.state", params);
  }
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a);
  out.set(b, a.length);
  return out;
}

async function writeAll(conn: Deno.TcpConn, bytes: Uint8Array) {
  let off = 0;
  while (off < bytes.length) off += await conn.write(bytes.subarray(off));
}
