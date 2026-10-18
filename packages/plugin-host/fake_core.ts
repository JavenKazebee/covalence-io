/**
 * A fake core for testing the host end to end: spawns the host process with
 * the coarse allowlist from README.md and speaks the protocol on its stdio.
 * Answers the host's `host.hello` and `ping` itself.
 *
 * @module
 */

import {
  encodeFrame,
  type Format,
  FrameDecoder,
  type HostHello,
  type Message,
  type Notification,
  type ProtocolError,
} from "@covalence-io/protocol";

const ROOT = new URL("../../", import.meta.url).pathname.replace(/\/$/, "");

/** The flags the core launches the host with (README.md). */
export function hostArgs(root = ROOT): string[] {
  return [
    "run",
    "--no-prompt",
    `--config=${root}/deno.json`,
    `--allow-read=${root}/packages,${root}/plugins`,
    "--allow-net",
    "--allow-env=COVALENCE_PROTOCOL_DEBUG",
    `${root}/packages/plugin-host/main.ts`,
  ];
}

export const REPO_ROOT = ROOT;

export class CoreError extends Error {
  constructor(readonly error: ProtocolError) {
    super(`${error.code}: ${error.message}`);
  }
}

export class FakeCore {
  readonly format: Format;
  /** The host's `host.hello` params, once received. */
  hello: HostHello | undefined;
  /** Every notification received, in order. */
  readonly notifications: Notification[] = [];
  readonly #child: Deno.ChildProcess;
  readonly #writer: WritableStreamDefaultWriter<Uint8Array>;
  readonly #pending = new Map<
    number,
    { resolve(r: unknown): void; reject(e: Error): void }
  >();
  readonly #wakers = new Set<() => void>();
  readonly #reading: Promise<void>;
  readonly #stderr: Promise<string>;
  #nextId = 1;
  #helloSeen!: () => void;
  readonly helloSeen: Promise<void> = new Promise((r) => this.#helloSeen = r);

  constructor(options: { format?: Format; term?: number } = {}) {
    this.format = options.format ?? "msgpack";
    const env: Record<string, string> = {};
    if (this.format === "json") env.COVALENCE_PROTOCOL_DEBUG = "json";
    this.#child = new Deno.Command("deno", {
      args: hostArgs(),
      cwd: ROOT,
      env,
      stdin: "piped",
      stdout: "piped",
      stderr: "piped",
    }).spawn();
    this.#writer = this.#child.stdin.getWriter();
    this.#stderr = new Response(this.#child.stderr).text();
    this.#reading = this.#read(options.term ?? 1);
  }

  async #read(term: number) {
    const decoder = new FrameDecoder(this.format);
    for await (const chunk of this.#child.stdout) {
      decoder.push(chunk);
      let msg: Message | undefined;
      while ((msg = decoder.next())) {
        if (msg.t === "req") {
          if (msg.m === "host.hello") {
            this.hello = msg.p as unknown as HostHello;
            this.#send({
              t: "res",
              id: msg.id,
              ok: true,
              r: { protocol: "0.1.0", session: "test", term, coreId: "core" },
            });
            this.#helloSeen();
          } else if (msg.m === "ping") {
            this.#send({ t: "res", id: msg.id, ok: true, r: {} });
          } else {
            this.#send({
              t: "res",
              id: msg.id,
              ok: false,
              e: { code: "unknown_method", message: msg.m },
            });
          }
        } else if (msg.t === "res") {
          const waiter = this.#pending.get(msg.id);
          this.#pending.delete(msg.id);
          if (msg.ok) waiter?.resolve(msg.r);
          else waiter?.reject(new CoreError(msg.e));
        } else {
          this.notifications.push(msg);
          for (const wake of this.#wakers) wake();
        }
      }
    }
  }

  #send(msg: Message) {
    this.#writer.write(encodeFrame(msg, this.format)).catch(() => {});
  }

  /** Sends a request to the host and waits for its result. */
  request(m: string, p: unknown, conn?: string): Promise<unknown> {
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      const msg: Message = { t: "req", id, m, p: p as Record<string, unknown> };
      if (conn !== undefined) msg.conn = conn;
      this.#send(msg);
    });
  }

  /**
   * Waits for a notification matching `match`, received after index `from`
   * (default: any so far). Returns it.
   */
  async waitFor(
    match: (n: Notification) => boolean,
    { from = 0, timeoutMs = 5000 } = {},
  ): Promise<Notification> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const hit = this.notifications.slice(from).find(match);
      if (hit) return hit;
      const left = deadline - Date.now();
      if (left <= 0) {
        throw new Error(
          `no matching notification; got ${
            JSON.stringify(this.notifications.slice(from))
          }`,
        );
      }
      await new Promise<void>((resolve) => {
        const done = () => {
          clearTimeout(timer);
          this.#wakers.delete(done);
          resolve();
        };
        const timer = setTimeout(done, left);
        this.#wakers.add(done);
      });
    }
  }

  /** Closes the host's stdin and waits for it to exit. */
  async close(): Promise<{ code: number; stderr: string }> {
    await this.#writer.close().catch(() => {});
    const status = await this.#child.status;
    await this.#reading;
    for (const w of this.#pending.values()) w.reject(new Error("host exited"));
    return { code: status.code, stderr: await this.#stderr };
  }

  /** Kills the host. */
  kill() {
    this.#child.kill("SIGKILL");
  }
}
