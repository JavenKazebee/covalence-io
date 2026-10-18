/**
 * The `deno-shared` host: speaks the plugin protocol with the core over a
 * byte stream (stdio in production) and runs one worker per connection
 * (ADR 0003).
 *
 * The main thread routes on the envelope's `conn` and passes params through
 * to the worker. The only params it reads are `action.invoke`'s `term`
 * (fencing is host-wide, spec §5.1) and `deadline`.
 *
 * @module
 */

import {
  type ActionInvoke,
  type ConnCreate,
  encodeFrame,
  type ErrorCode,
  type Format,
  FrameDecoder,
  FrameError,
  type HostHello,
  type HostWelcome,
  type LogLevel,
  type Manifest,
  type Message,
  type PluginLoad,
  PROTOCOL_VERSION,
  type ProtocolError,
  type Request,
} from "@covalence-io/protocol";
import type {
  FromWorker,
  ToWorker,
  WorkerCreate,
  WorkerMethod,
} from "./worker_messages.ts";

export const HOST_VERSION = "0.1.0";

export interface HostOptions {
  readable: ReadableStream<Uint8Array>;
  writable: WritableStream<Uint8Array>;
  format: Format;
  /** Keepalive interval (spec §2). Default 2000 ms. */
  pingMs?: number;
  /**
   * Called when the link ends (EOF, `host.shutdown` or a dead link), after
   * the last message is written.
   */
  onExit?: (code: number) => void;
}

class RequestFailed extends Error {
  constructor(readonly code: ErrorCode, message: string) {
    super(message);
  }
}

interface LoadedPlugin {
  bundle: string;
  manifest: Manifest;
}

/** A connection and its worker. */
class Connection {
  readonly #worker: Worker;
  readonly #pending = new Map<
    number,
    { resolve(r: unknown): void; reject(e: ProtocolError): void }
  >();
  #nextId = 1;
  crashed: string | undefined;

  constructor(
    readonly id: string,
    onNotify: (m: string, p: unknown) => void,
    onCrash: (why: string) => void,
  ) {
    this.#worker = new Worker(new URL("./worker.ts", import.meta.url), {
      type: "module",
      name: `conn:${id}`,
    });
    this.#worker.onmessage = (e: MessageEvent<FromWorker>) => {
      const msg = e.data;
      if (msg.type === "ntf") return onNotify(msg.m, msg.p);
      const waiter = this.#pending.get(msg.id);
      if (!waiter) return; // gave up on it (deadline)
      this.#pending.delete(msg.id);
      if (msg.ok) waiter.resolve(msg.r);
      else waiter.reject(msg.e);
    };
    this.#worker.onerror = (e: ErrorEvent) => {
      e.preventDefault(); // keep the host and the other connections alive
      this.#fail(`worker crashed: ${e.message}`);
      onCrash(this.crashed!);
    };
    this.#worker.onmessageerror = () => {
      this.#fail("worker sent an unreadable message");
      onCrash(this.crashed!);
    };
  }

  /** Sends a request to the worker. `id` lets the caller give up on it. */
  request(
    m: WorkerMethod,
    p: unknown,
  ): { id: number; result: Promise<unknown> } {
    const id = this.#nextId++;
    const result = new Promise<unknown>((resolve, reject) => {
      if (this.crashed) {
        return reject({ code: "not_connected", message: this.crashed });
      }
      this.#pending.set(id, { resolve, reject });
      this.#post({ type: "req", id, m, p });
    });
    return { id, result };
  }

  /** Stops waiting for a request and tells the worker to abort the call. */
  abandon(id: number, call: string) {
    this.#pending.delete(id);
    this.#post({ type: "cancel", call });
  }

  terminate() {
    this.#worker.terminate();
    this.#fail("connection destroyed");
  }

  #post(msg: ToWorker) {
    if (!this.crashed) this.#worker.postMessage(msg);
  }

  #fail(why: string) {
    this.crashed ??= why;
    this.#worker.terminate();
    for (const w of this.#pending.values()) {
      w.reject({ code: "not_connected", message: why });
    }
    this.#pending.clear();
  }
}

export class Host {
  readonly #opts: HostOptions;
  readonly #writer: WritableStreamDefaultWriter<Uint8Array>;
  /** Loaded (or loading) plugins by `id@version`. */
  readonly #plugins = new Map<string, Promise<LoadedPlugin>>();
  readonly #conns = new Map<string, Connection>();
  /** Connections whose conn.create is in progress. */
  readonly #creating = new Map<string, Promise<unknown>>();
  readonly #pending = new Map<
    number,
    { resolve(r: unknown): void; reject(e: ProtocolError): void }
  >();
  #nextId = 1;
  #term = 0;
  #unansweredPings = 0;
  #pingTimer: ReturnType<typeof setInterval> | undefined;
  #closed = false;
  #lastWrite: Promise<void> = Promise.resolve();

  constructor(opts: HostOptions) {
    this.#opts = opts;
    this.#writer = opts.writable.getWriter();
  }

  /** Runs until the link ends. Resolves to the exit code. */
  async run(): Promise<number> {
    this.#hello();
    this.#pingTimer = setInterval(
      () => this.#ping(),
      this.#opts.pingMs ?? 2000,
    );
    const decoder = new FrameDecoder(this.#opts.format);
    try {
      for await (const chunk of this.#opts.readable) {
        decoder.push(chunk);
        for (;;) {
          let msg: Message | undefined;
          try {
            msg = decoder.next();
          } catch (err) {
            if (err instanceof FrameError && !err.fatal) {
              this.#log("warn", `dropped a bad frame: ${err.message}`);
              continue;
            }
            throw err;
          }
          if (!msg) break;
          this.#dispatch(msg);
        }
        if (this.#closed) break;
      }
    } catch (err) {
      this.#log(
        "error",
        `link failed: ${err instanceof Error ? err.message : err}`,
      );
      return this.#shutdown(1);
    }
    return this.#shutdown(0);
  }

  #dispatch(msg: Message) {
    switch (msg.t) {
      case "req":
        this.#onRequest(msg);
        break;
      case "res": {
        const waiter = this.#pending.get(msg.id);
        if (!waiter) return;
        this.#pending.delete(msg.id);
        if (msg.ok) waiter.resolve(msg.r);
        else waiter.reject(msg.e);
        break;
      }
      case "ntf":
        break; // the core sends none we handle yet; unknown ones are ignored
    }
  }

  async #onRequest(req: Request) {
    try {
      const r = await this.#handle(req);
      this.#send({ t: "res", id: req.id, ok: true, r: r ?? {} });
    } catch (err) {
      this.#send({ t: "res", id: req.id, ok: false, e: toProtocolError(err) });
    }
  }

  // deno-lint-ignore no-explicit-any
  async #handle(req: Request): Promise<any> {
    switch (req.m) {
      case "ping":
        return {};
      case "plugin.load":
        return {
          manifest: (await this.#load(req.p as unknown as PluginLoad))
            .manifest,
        };
      case "conn.create": {
        const id = need(req.conn);
        if (this.#creating.has(id)) {
          throw new RequestFailed("invalid_request", `connection ${id} exists`);
        }
        const creating = this.#create(id, req.p as unknown as ConnCreate);
        this.#creating.set(id, creating);
        try {
          return await creating;
        } finally {
          this.#creating.delete(id);
        }
      }
      case "conn.destroy": {
        const conn = await this.#conn(req.conn);
        this.#conns.delete(conn.id);
        try {
          await withTimeout(conn.request("conn.destroy", req.p).result, 2000);
        } catch {
          // destroy anyway
        }
        conn.terminate();
        return {};
      }
      case "conn.resync":
        return await (await this.#conn(req.conn)).request("conn.resync", req.p)
          .result;
      case "action.invoke":
        return await this.#invoke(
          await this.#conn(req.conn),
          req.p as unknown as ActionInvoke,
        );
      case "host.shutdown":
        setTimeout(() => this.#shutdown(0), 0); // after the response
        return {};
      default:
        throw new RequestFailed("unknown_method", `unknown method ${req.m}`);
    }
  }

  // A conn.create that arrives while its plugin is still loading waits for it.
  #load(p: PluginLoad): Promise<LoadedPlugin> {
    const key = `${p.plugin}@${p.version}`;
    const loading = this.#import(p);
    this.#plugins.set(key, loading);
    loading.catch(() => {
      if (this.#plugins.get(key) === loading) this.#plugins.delete(key);
    });
    return loading;
  }

  async #import(p: PluginLoad): Promise<LoadedPlugin> {
    const bundle = toUrl(p.bundle);
    const mod = await import(bundle);
    const manifest = mod.default?.manifest as Manifest | undefined;
    if (!manifest || !mod.default.definition) {
      throw new RequestFailed(
        "invalid_params",
        `${p.bundle} has no definePlugin() default export`,
      );
    }
    if (manifest.id !== p.plugin || manifest.version !== p.version) {
      throw new RequestFailed(
        "invalid_params",
        `${p.bundle} is ${manifest.id}@${manifest.version}, not ${p.plugin}@${p.version}`,
      );
    }
    return { bundle, manifest };
  }

  async #create(id: string, p: ConnCreate) {
    const plugin = await this.#plugins.get(`${p.plugin}@${p.version}`)
      ?.catch(() => undefined);
    if (!plugin) {
      throw new RequestFailed(
        "not_found",
        `${p.plugin}@${p.version} is not loaded`,
      );
    }
    if (this.#conns.has(id)) {
      throw new RequestFailed("invalid_request", `connection ${id} exists`);
    }
    // ADR 0015: p.permissions is not applied; workers inherit the host's.
    const conn = new Connection(
      id,
      (m, params) => this.#send({ t: "ntf", m, conn: id, p: params }),
      (why) => {
        this.#log("error", why, id);
        this.#send({
          t: "ntf",
          m: "conn.status",
          conn: id,
          p: { status: "error", message: why },
        });
      },
    );
    this.#conns.set(id, conn);
    const create: WorkerCreate = {
      bundle: plugin.bundle,
      conn: id,
      params: {
        alias: p.alias,
        mode: p.mode,
        config: p.config,
        secrets: p.secrets,
      },
    };
    try {
      await conn.request("conn.create", create).result;
    } catch (err) {
      this.#conns.delete(id);
      conn.terminate();
      throw err;
    }
    return {};
  }

  async #invoke(conn: Connection, p: ActionInvoke) {
    if (typeof p.term !== "number") {
      throw new RequestFailed("invalid_params", "term is required");
    }
    if (p.term < this.#term) {
      throw new RequestFailed(
        "stale_term",
        `term ${p.term} is older than ${this.#term}`,
      );
    }
    this.#term = p.term;
    const { id, result } = conn.request("action.invoke", p);
    if (typeof p.deadline !== "number") return await result;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const expired = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        conn.abandon(id, p.call);
        reject(
          new RequestFailed("timeout", `deadline of ${p.deadline} ms passed`),
        );
      }, p.deadline);
    });
    try {
      return await Promise.race([result, expired]);
    } finally {
      clearTimeout(timer);
    }
  }

  async #conn(id: string | undefined): Promise<Connection> {
    await this.#creating.get(need(id))?.catch(() => {});
    const conn = this.#conns.get(id!);
    if (!conn) throw new RequestFailed("not_found", `no connection ${id}`);
    return conn;
  }

  async #hello() {
    const hello: HostHello = {
      protocol: { min: PROTOCOL_VERSION, max: "0.1.x" },
      host: {
        kind: "deno-shared",
        version: HOST_VERSION,
        runtime: `deno ${Deno.version.deno}`,
      },
      capabilities: ["workers", "simulate"],
    };
    try {
      const welcome = await this.#request("host.hello", hello) as HostWelcome;
      this.#term = Math.max(this.#term, welcome.term);
      if (welcome.protocol.split(".").slice(0, 2).join(".") !== "0.1") {
        this.#log("error", `core chose protocol ${welcome.protocol}`);
        this.#shutdown(1);
      }
    } catch (err) {
      this.#log("error", `host.hello refused: ${toProtocolError(err).message}`);
      this.#shutdown(1);
    }
  }

  #ping() {
    if (this.#unansweredPings >= 3) {
      this.#log("error", "core stopped answering pings");
      this.#shutdown(1);
      return;
    }
    this.#unansweredPings++;
    this.#request("ping", {}).then(
      () => this.#unansweredPings = 0,
      () => {},
    );
  }

  #request(m: string, p: unknown): Promise<unknown> {
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      this.#send({ t: "req", id, m, p: p as Request["p"] });
    });
  }

  #log(level: LogLevel, msg: string, conn?: string) {
    this.#send({ t: "ntf", m: "log", conn, p: { level, msg } });
  }

  #send(msg: Message) {
    if (this.#closed) return;
    try {
      this.#lastWrite = this.#writer.write(encodeFrame(msg, this.#opts.format))
        .catch(() => {});
    } catch (err) {
      console.error("plugin host: could not encode a message", err);
    }
  }

  #shutdown(code: number): number {
    if (this.#closed) return code;
    this.#closed = true;
    clearInterval(this.#pingTimer);
    for (const conn of this.#conns.values()) conn.terminate();
    this.#conns.clear();
    for (const w of this.#pending.values()) {
      w.reject({ code: "cancelled", message: "host shutting down" });
    }
    this.#pending.clear();
    this.#lastWrite.finally(() => this.#opts.onExit?.(code));
    return code;
  }
}

function need(conn: string | undefined): string {
  if (!conn) {
    throw new RequestFailed("invalid_request", "missing envelope conn");
  }
  return conn;
}

function toProtocolError(err: unknown): ProtocolError {
  if (err instanceof RequestFailed) {
    return { code: err.code, message: err.message };
  }
  if (
    err && typeof err === "object" && "code" in err && "message" in err &&
    typeof err.code === "string"
  ) {
    return err as ProtocolError; // from a worker
  }
  return {
    code: "internal",
    message: err instanceof Error ? err.message : String(err),
  };
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout>;
  return Promise.race([
    p,
    new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error("timed out")), ms);
    }),
  ]).finally(() => clearTimeout(timer));
}

/** A bundle path or URL as an import specifier. */
export function toUrl(bundle: string): string {
  if (/^[a-z][a-z0-9+.-]+:/i.test(bundle) && !/^[a-z]:[\\/]/i.test(bundle)) {
    return bundle; // already a URL (file:, https:, jsr:, …)
  }
  const path = bundle.replaceAll("\\", "/");
  if (path.startsWith("/")) return new URL(`file://${path}`).href;
  if (/^[a-z]:\//i.test(path)) return new URL(`file:///${path}`).href;
  return new URL(path, `file://${Deno.cwd()}/`).href;
}
