/**
 * Host transports (plugin protocol §7): I/O the core performs for the
 * connection. Hardware always goes through the core; network I/O may, so the
 * core can show it in the inspector and reconnect it.
 *
 * - `ctx.hid.open(device)` and `ctx.serial.open(device)` open a device by the
 *   ID in the connection's config (a string field of `format: "device"`).
 *   Handles survive unplugging: they report `closed` with `unplugged`, and
 *   `open` again when the device returns.
 * - `ctx.tcp.connect({hostname, port})` gives an object shaped like
 *   `Deno.Conn`; `ctx.net.socket()` one shaped like a `node:net` `Socket`.
 *   Libraries that accept a socket can use either unchanged.
 * - `ctx.devices.list()` lists the devices this plugin's `devices` match.
 *
 * @module
 */

import { EventEmitter } from "node:events";
import { Buffer } from "node:buffer";
import type {
  Bytes,
  DeviceInfo,
  DeviceKind,
  Framing,
  IoKind,
  IoOptions,
  IoStateKind,
} from "@covalence-io/protocol";

/** How transports reach the core (the runtime's sink). */
export interface IoSink {
  notify(method: string, params: unknown): void;
  request(method: string, params: unknown): Promise<unknown>;
}

/** Bytes as they arrive: `bin` (MessagePack) or numbers (the JSON debug format). */
export function toBytes(data: Bytes | ArrayBufferView | string): Uint8Array {
  if (typeof data === "string") return new TextEncoder().encode(data);
  if (data instanceof Uint8Array) return data;
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  return Uint8Array.from(data);
}

type Listener<A extends unknown[]> = (...args: A) => void;

/** A handle the core holds open for this connection. */
export class IoHandle {
  #state: IoStateKind = "open";
  #error: string | undefined;
  #closed = false;
  readonly #data = new Set<Listener<[Uint8Array, number | undefined]>>();
  readonly #states = new Set<Listener<[IoStateKind, string | undefined]>>();
  readonly #io: Transports;

  constructor(
    readonly handle: string,
    readonly kind: IoKind,
    readonly target: string,
    io: Transports,
  ) {
    this.#io = io;
  }

  /** The connection's transports. */
  protected get transports(): Transports {
    return this.#io;
  }

  /** `open`, `closed`, `reconnecting` or `error`. */
  get state(): IoStateKind {
    return this.#state;
  }

  /** Why it isn't open: `unplugged`, `permission_denied`, … */
  get error(): string | undefined {
    return this.#error;
  }

  /** Whether `close()` was called, or the core closed it for good. */
  get closed(): boolean {
    return this.#closed;
  }

  /**
   * Calls `fn` with each input report or chunk, and when the core read it
   * (ms since the epoch, the core's clock). Returns an unsubscribe.
   */
  onData(fn: Listener<[Uint8Array, number | undefined]>): () => void {
    this.#data.add(fn);
    return () => this.#data.delete(fn);
  }

  /** Calls `fn` when the handle opens, closes or fails. */
  onState(fn: Listener<[IoStateKind, string | undefined]>): () => void {
    this.#states.add(fn);
    return () => this.#states.delete(fn);
  }

  /** Sends bytes; for HID, an output report whose first byte is its ID. */
  write(data: Bytes | ArrayBufferView | string) {
    if (this.#closed) throw new Error(`handle ${this.handle} is closed`);
    this.#io.sink.notify("io.write", {
      handle: this.handle,
      data: toBytes(data),
    });
  }

  close() {
    if (this.#closed) return;
    this.#io.sink.notify("io.close", { handle: this.handle });
    this.#end();
  }

  /** @internal */
  _data(data: Uint8Array, ts?: number) {
    for (const fn of this.#data) fn(data, ts);
  }

  /** @internal */
  _state(state: IoStateKind, error: string | undefined) {
    this.#state = state;
    this.#error = error;
    for (const fn of this.#states) fn(state, error);
    // A socket that won't reconnect is gone; a device handle never is.
    if (state === "closed" && this.kind === "tcp") this.#end();
  }

  #end() {
    if (this.#closed) return;
    this.#closed = true;
    this.#io._forget(this.handle);
  }
}

/** A HID device the core opened (`ctx.hid.open`). */
export class HidDevice extends IoHandle {
  /** Calls `fn` with each input report and when the core read it. */
  onInput(fn: Listener<[Uint8Array, number | undefined]>): () => void {
    return this.onData(fn);
  }

  /** Sends a feature report; its first byte is the report ID. */
  async sendFeatureReport(data: Bytes | ArrayBufferView): Promise<void> {
    await this.transports.sink.request("io.feature", {
      handle: this.handle,
      op: "send",
      data: toBytes(data),
    });
  }

  /** Reads feature report `reportId`: up to `length` bytes, the ID first. */
  async getFeatureReport(
    reportId: number,
    length: number,
  ): Promise<Uint8Array> {
    const r = await this.transports.sink.request("io.feature", {
      handle: this.handle,
      op: "get",
      reportId,
      length,
    }) as { data?: Bytes };
    return toBytes(r.data ?? []);
  }
}

/** A serial port the core opened (`ctx.serial.open`). */
export class SerialPort extends IoHandle {
  #readable: ReadableStream<Uint8Array> | undefined;

  /** The bytes read, as a stream. Ends when the port is closed. */
  get readable(): ReadableStream<Uint8Array> {
    this.#readable ??= streamOf(this);
    return this.#readable;
  }
}

function streamOf(handle: IoHandle): ReadableStream<Uint8Array> {
  let off: (() => void)[] = [];
  return new ReadableStream<Uint8Array>({
    start(controller) {
      off = [
        handle.onData((d) => controller.enqueue(d)),
        handle.onState(() => {
          if (handle.closed) {
            off.forEach((f) => f());
            try {
              controller.close();
            } catch { /* already closed */ }
          }
        }),
      ];
    },
    cancel() {
      off.forEach((f) => f());
    },
  });
}

export interface TcpConnectOptions {
  hostname?: string;
  port: number;
  /** Reconnect with backoff when the peer goes, instead of closing. */
  reconnect?: boolean;
  backoff?: { initialMs?: number; maxMs?: number };
  /** How the core cuts what it reads into messages. Default `raw`. */
  framing?: Framing;
}

export interface NetAddr {
  transport: "tcp";
  hostname: string;
  port: number;
}

/**
 * A TCP connection through the core, shaped like `Deno.Conn`: `read`,
 * `write`, `readable`, `writable`, `close`. With `reconnect`, it lives across
 * reconnects; `onState` says when it's up.
 *
 * `onState` keeps its place among the bytes: a change fires once everything
 * that arrived before it has been read, so a reader that resets its parser on
 * `open` never feeds it the last link's bytes. A change that comes before
 * anything listens (the first `open` can beat `connect`'s answer) waits for
 * the next `read` too.
 */
export class TcpConn {
  readonly #handle: IoHandle;
  /** Bytes, and state changes still behind unread bytes. */
  readonly #queue: (Uint8Array | [IoStateKind, string | undefined])[] = [];
  readonly #states = new Set<Listener<[IoStateKind, string | undefined]>>();
  #waiting: (() => void) | undefined;
  #readable: ReadableStream<Uint8Array> | undefined;
  #writable: WritableStream<Uint8Array> | undefined;
  readonly localAddr: NetAddr;
  readonly remoteAddr: NetAddr;

  constructor(handle: IoHandle, hostname: string, port: number) {
    this.#handle = handle;
    this.localAddr = { transport: "tcp", hostname: "0.0.0.0", port: 0 };
    this.remoteAddr = { transport: "tcp", hostname, port };
    handle.onData((d) => {
      this.#queue.push(d);
      this.#wake();
    });
    handle.onState((state, error) => {
      if (this.#queue.length === 0 && this.#states.size > 0) {
        this.#fire(state, error);
      } else this.#queue.push([state, error]);
      this.#wake();
    });
  }

  /** The core's handle, for state and the inspector. */
  get handle(): IoHandle {
    return this.#handle;
  }

  /**
   * Calls `fn` when the socket opens, closes or starts reconnecting, after
   * the bytes before that have been read. Returns an unsubscribe.
   */
  onState(fn: Listener<[IoStateKind, string | undefined]>): () => void {
    this.#states.add(fn);
    return () => this.#states.delete(fn);
  }

  #fire(state: IoStateKind, error: string | undefined) {
    for (const fn of this.#states) fn(state, error);
  }

  /** Fires the state changes at the head of the queue. */
  #flush() {
    while (this.#queue.length > 0 && !(this.#queue[0] instanceof Uint8Array)) {
      const [state, error] = this.#queue.shift() as [
        IoStateKind,
        string | undefined,
      ];
      this.#fire(state, error);
    }
  }

  #wake() {
    const w = this.#waiting;
    this.#waiting = undefined;
    w?.();
  }

  /** Reads into `p`. Resolves to the bytes read, or `null` once closed. */
  async read(p: Uint8Array): Promise<number | null> {
    for (;;) {
      this.#flush();
      if (this.#queue.length > 0) break;
      if (this.#handle.closed) return null;
      await new Promise<void>((resolve) => this.#waiting = resolve);
    }
    const chunk = this.#queue[0] as Uint8Array;
    const n = Math.min(p.length, chunk.length);
    p.set(chunk.subarray(0, n));
    if (n === chunk.length) {
      this.#queue.shift();
      this.#flush();
    } else this.#queue[0] = chunk.subarray(n);
    return n;
  }

  /** Sends all of `p`. The core writes in order. */
  write(p: Uint8Array): Promise<number> {
    this.#handle.write(p);
    return Promise.resolve(p.length);
  }

  get readable(): ReadableStream<Uint8Array> {
    this.#readable ??= new ReadableStream<Uint8Array>({
      pull: async (controller) => {
        const buf = new Uint8Array(64 * 1024);
        const n = await this.read(buf);
        if (n === null) controller.close();
        else controller.enqueue(buf.slice(0, n));
      },
    });
    return this.#readable;
  }

  get writable(): WritableStream<Uint8Array> {
    this.#writable ??= new WritableStream<Uint8Array>({
      write: (chunk) => {
        this.#handle.write(chunk);
      },
      close: () => this.close(),
    });
    return this.#writable;
  }

  close() {
    this.#handle.close();
    this.#wake();
  }

  /** Not separable through the core: closes both directions. */
  closeWrite(): Promise<void> {
    this.close();
    return Promise.resolve();
  }

  /** The core sets no-delay on every socket. */
  setNoDelay(_noDelay?: boolean) {}
  setKeepAlive(_keepAlive?: boolean) {}
  ref() {}
  unref() {}

  [Symbol.dispose]() {
    this.close();
  }
}

/**
 * A socket through the core, shaped like a `node:net` `Socket`: `connect`,
 * `write`, `end`, `destroy`, `setEncoding`, `setTimeout`, and the `connect`,
 * `ready`, `data`, `end`, `close`, `error` and `timeout` events.
 */
export class NetSocket extends EventEmitter {
  #io: Transports;
  #handle: IoHandle | undefined;
  #encoding: string | undefined;
  #timeoutMs = 0;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #queued: Uint8Array[] = [];
  destroyed = false;
  connecting = false;
  remoteAddress: string | undefined;
  remotePort: number | undefined;

  constructor(io: Transports) {
    super();
    this.#io = io;
  }

  get readyState(): "opening" | "open" | "closed" {
    if (this.destroyed) return "closed";
    return this.#handle?.state === "open" ? "open" : "opening";
  }

  connect(
    portOrOptions: number | { port: number; host?: string },
    hostOrListener?: string | (() => void),
    listener?: () => void,
  ): this {
    const port = typeof portOrOptions === "number"
      ? portOrOptions
      : portOrOptions.port;
    const host = typeof portOrOptions === "number"
      ? (typeof hostOrListener === "string" ? hostOrListener : "localhost")
      : portOrOptions.host ?? "localhost";
    const done = typeof hostOrListener === "function"
      ? hostOrListener
      : listener;
    if (done) this.once("connect", done);
    this.connecting = true;
    this.remoteAddress = host;
    this.remotePort = port;
    this.#io.open("tcp", `${host}:${port}`).then((handle) => {
      this.connecting = false;
      if (this.destroyed) return handle.close();
      this.#handle = handle;
      handle.onData((d) => {
        this.#touch();
        this.emit(
          "data",
          this.#encoding
            ? new TextDecoder(this.#encoding).decode(d)
            : Buffer.from(d),
        );
      });
      handle.onState((state, error) => {
        if (state === "open") return;
        this.emit("end");
        this.#finish(
          error && error !== "closed by the peer" ? error : undefined,
        );
      });
      for (const d of this.#queued) handle.write(d);
      this.#queued = [];
      this.#touch();
      this.emit("connect");
      this.emit("ready");
    }, (err) => {
      this.connecting = false;
      this.#finish(err?.message ?? String(err));
    });
    return this;
  }

  write(
    data: string | Uint8Array,
    encodingOrCb?: string | (() => void),
    cb?: () => void,
  ): boolean {
    if (this.destroyed) return false;
    const bytes = typeof data === "string" ? toBytes(data) : data;
    if (this.#handle) this.#handle.write(bytes);
    else this.#queued.push(bytes);
    this.#touch();
    const done = typeof encodingOrCb === "function" ? encodingOrCb : cb;
    if (done) queueMicrotask(done);
    return true;
  }

  end(data?: string | Uint8Array, cb?: () => void): this {
    if (data !== undefined) this.write(data);
    this.#finish(undefined);
    if (cb) queueMicrotask(cb);
    return this;
  }

  destroy(error?: Error): this {
    this.#finish(error?.message);
    return this;
  }

  setEncoding(encoding: string): this {
    this.#encoding = encoding;
    return this;
  }

  setTimeout(ms: number, cb?: () => void): this {
    this.#timeoutMs = ms;
    if (cb) this.once("timeout", cb);
    this.#touch();
    return this;
  }

  setNoDelay(_noDelay?: boolean): this {
    return this;
  }

  setKeepAlive(_enable?: boolean, _delay?: number): this {
    return this;
  }

  ref(): this {
    return this;
  }

  unref(): this {
    return this;
  }

  address(): { address: string; family: string; port: number } {
    return { address: "0.0.0.0", family: "IPv4", port: 0 };
  }

  #touch() {
    clearTimeout(this.#timer);
    if (this.#timeoutMs > 0 && !this.destroyed) {
      this.#timer = setTimeout(() => this.emit("timeout"), this.#timeoutMs);
    }
  }

  #finish(error: string | undefined) {
    if (this.destroyed) return;
    this.destroyed = true;
    clearTimeout(this.#timer);
    this.#handle?.close();
    if (error !== undefined) this.emit("error", new Error(error));
    this.emit("close", error !== undefined);
  }
}

/** The transports of one connection. The runtime makes one. */
export class Transports {
  readonly #handles = new Map<string, IoHandle>();
  /** Events for handles not known yet: `io.open`'s answer is still on its way. */
  readonly #early = new Map<string, [string, unknown][]>();

  constructor(readonly sink: IoSink) {}

  /** Opens a handle. Device handles open even when the device is away. */
  async open(
    kind: IoKind,
    target: string,
    options?: IoOptions,
    wrap?: (handle: IoHandle) => void,
  ): Promise<IoHandle> {
    const params: Record<string, unknown> = { kind, target };
    if (options) params.options = options;
    const { handle } = await this.sink.request("io.open", params) as {
      handle: string;
    };
    const io = kind === "hid"
      ? new HidDevice(handle, kind, target, this)
      : kind === "serial"
      ? new SerialPort(handle, kind, target, this)
      : new IoHandle(handle, kind, target, this);
    this.#handles.set(handle, io);
    // The wrapper listens before the events that beat the answer replay.
    wrap?.(io);
    for (const [m, p] of this.#early.get(handle) ?? []) this.receive(m, p);
    this.#early.delete(handle);
    return io;
  }

  /**
   * Handles `io.data` and `io.state` from the core. Returns whether it was
   * one of them.
   */
  receive(method: string, params: unknown): boolean {
    if (method !== "io.data" && method !== "io.state") return false;
    const p = params as {
      handle: string;
      data?: Bytes;
      ts?: number;
      state?: IoStateKind;
      error?: string;
    };
    const handle = this.#handles.get(p.handle);
    if (!handle) {
      const early = this.#early.get(p.handle) ?? [];
      if (early.length < 64) early.push([method, params]);
      this.#early.set(p.handle, early);
      return true;
    }
    if (method === "io.data") handle._data(toBytes(p.data ?? []), p.ts);
    else handle._state(p.state!, p.error);
    return true;
  }

  /** Closes every handle: the connection is going. */
  closeAll() {
    for (const handle of [...this.#handles.values()]) handle.close();
    this.#early.clear();
  }

  /** @internal */
  _forget(handle: string) {
    this.#handles.delete(handle);
  }

  // The `ctx` APIs.

  readonly hid = {
    /** Opens a HID device by its ID. */
    open: (device: string): Promise<HidDevice> =>
      this.open("hid", device) as Promise<HidDevice>,
  };

  readonly serial = {
    /** Opens a serial port by its device ID. Default 115200 baud. */
    open: (device: string, options?: { baud?: number }): Promise<SerialPort> =>
      this.open("serial", device, options) as Promise<SerialPort>,
  };

  readonly tcp = {
    /**
     * Connects through the core. Rejects if the first attempt fails, unless
     * `reconnect` is set.
     */
    connect: async (options: TcpConnectOptions): Promise<TcpConn> => {
      const hostname = options.hostname ?? "127.0.0.1";
      const io: IoOptions = {};
      if (options.reconnect !== undefined) io.reconnect = options.reconnect;
      if (options.backoff) io.backoff = options.backoff;
      if (options.framing) io.framing = options.framing;
      let conn!: TcpConn;
      await this.open(
        "tcp",
        `${hostname}:${options.port}`,
        io,
        (handle) => conn = new TcpConn(handle, hostname, options.port),
      );
      return conn;
    },
  };

  readonly net = {
    /** A `node:net`-shaped socket through the core. Call `connect`. */
    socket: (): NetSocket => new NetSocket(this),
    /** Like `net.createConnection(port, host, listener)`. */
    createConnection: (
      port: number,
      host?: string,
      listener?: () => void,
    ): NetSocket =>
      new NetSocket(this).connect(port, host ?? "localhost", listener),
  };

  readonly devices = {
    /** Devices this plugin's `devices` match, present now. */
    list: (kind?: DeviceKind): Promise<DeviceInfo[]> =>
      this.sink.request(
        "device.list",
        kind ? { kind } : {},
      ) as Promise<DeviceInfo[]>,
  };
}
