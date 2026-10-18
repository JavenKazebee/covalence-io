/**
 * Runs one connection of a plugin: builds the hook context, keeps the state
 * mirror, coalesces patches and turns everything into protocol notifications.
 *
 * The plugin host runs one of these per worker. Plugin tests can run one
 * in-process with a sink that records what would go to the core.
 *
 * @module
 */

import type {
  ActionInvoke,
  ActionResult,
  ConnMode,
  ConnStatusKind,
  LogLevel,
  PatchOp,
} from "@covalence-io/protocol";
import { type Path, type PathLike, toPath } from "./paths.ts";
import {
  ActionError,
  type BaseContext,
  type CallInfo,
  type Context,
  type Logger,
  type Plugin,
  type StateApi,
} from "./plugin.ts";
import { validate, withDefaults } from "./schema.ts";
import { PatchCoalescer, StateTree } from "./state.ts";

/** Where the runtime sends host → core notifications (without `conn`). */
export interface RuntimeSink {
  notify(method: string, params: unknown): void;
}

export interface ConnectionInit {
  conn?: string;
  alias: string;
  mode: ConnMode;
  config: Record<string, unknown>;
  secrets?: Record<string, string | undefined>;
}

export interface RuntimeOptions {
  /** Patch coalescing window in ms (spec §6). Default 1. */
  coalesceMs?: number;
}

export class ConnectionRuntime {
  readonly #plugin: Plugin;
  readonly #init: ConnectionInit;
  readonly #sink: RuntimeSink;
  readonly #coalesceMs: number;
  readonly #tree = new StateTree();
  readonly #pending = new PatchCoalescer();
  readonly #abort = new AbortController();
  readonly #ctx: BaseContext<unknown, Record<string, unknown>>;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #status: ConnStatusKind | undefined;
  #snapshotSent = false;
  #started: Promise<boolean> | undefined;
  #session: unknown;
  #destroyed = false;

  constructor(
    plugin: Plugin,
    init: ConnectionInit,
    sink: RuntimeSink,
    options: RuntimeOptions = {},
  ) {
    this.#plugin = plugin;
    this.#init = init;
    this.#sink = sink;
    this.#coalesceMs = options.coalesceMs ?? 1;
    this.#ctx = this.#makeContext();
  }

  /** The context hooks receive; exposed for tests. */
  get context(): BaseContext<unknown, Record<string, unknown>> {
    return this.#ctx;
  }

  /**
   * Connects (live) or initialises the simulator. Resolves to whether the
   * connection came up; failures are reported as `conn.status` `error`.
   */
  start(): Promise<boolean> {
    this.#started ??= this.#start();
    return this.#started;
  }

  async #start(): Promise<boolean> {
    const def = this.#plugin.definition;
    this.#ctx.status("connecting");
    try {
      if (this.#init.mode === "simulate") {
        await def.simulator?.init?.(this.#ctx);
      } else {
        const problem = validate(def.config, this.#ctx.config, "config");
        if (problem) throw new Error(problem);
        this.#session = await def.connect?.(this.#ctx);
      }
    } catch (err) {
      if (!this.#destroyed) this.#ctx.status("error", errorMessage(err));
      return false;
    }
    if (this.#destroyed) return false;
    // A status change to ok sends the snapshot first.
    if (this.#status === "connecting") this.#ctx.status("ok");
    return true;
  }

  /** Runs an action (spec §5.1). Throws {@linkcode ActionError}. */
  async invoke(
    invoke: Omit<ActionInvoke, "term">,
    signal?: AbortSignal,
  ): Promise<ActionResult> {
    const def = this.#plugin.definition;
    const spec = def.actions[invoke.action];
    if (!spec) {
      throw new ActionError("not_found", `unknown action ${invoke.action}`);
    }
    const problem = validate(spec.params, invoke.params, "params");
    if (problem) throw new ActionError("invalid_params", problem);

    const up = await this.start();
    if (this.#destroyed) {
      throw new ActionError("not_connected", "connection destroyed");
    }
    const call: CallInfo = {
      call: invoke.call,
      run: invoke.run,
      signal: signal
        ? AbortSignal.any([signal, this.#abort.signal])
        : this.#abort.signal,
    };
    let result: unknown;
    try {
      if (this.#init.mode === "simulate") {
        const handler = def.simulator?.actions?.[invoke.action];
        if (!handler) {
          throw new ActionError(
            "action_failed",
            `${invoke.action} is not simulated`,
          );
        }
        result = await handler(this.#ctx, invoke.params, call);
      } else {
        if (!up) throw new ActionError("not_connected", "not connected");
        const ctx: Context = Object.assign(Object.create(this.#ctx), {
          session: this.#session,
        });
        result = await spec.run(ctx, invoke.params, call);
      }
    } catch (err) {
      if (err instanceof ActionError) throw err;
      if (call.signal.aborted) {
        throw new ActionError("cancelled", errorMessage(err));
      }
      throw new ActionError("action_failed", errorMessage(err));
    } finally {
      this.flush(); // state lands before the result
    }
    return result === undefined ? {} : { result };
  }

  /** Sends a fresh `state.snapshot` (spec §3.3 `conn.resync`). */
  resync() {
    this.#sendSnapshot();
  }

  /** Disconnects and stops sending. */
  async destroy(reason: string) {
    if (this.#destroyed) return;
    this.#destroyed = true;
    this.#abort.abort(new Error(`connection destroyed: ${reason}`));
    const def = this.#plugin.definition;
    if (this.#init.mode === "live" && def.disconnect) {
      // Wait for connect to settle so disconnect sees the session.
      await this.#started?.catch(() => {});
      try {
        const ctx: Context = Object.assign(Object.create(this.#ctx), {
          session: this.#session,
        });
        await def.disconnect(ctx, reason);
      } catch (err) {
        this.#log("warn", `disconnect failed: ${errorMessage(err)}`);
      }
    }
    clearTimeout(this.#timer);
    this.#pending.clear();
  }

  /** Sends pending patches now. */
  flush() {
    clearTimeout(this.#timer);
    this.#timer = undefined;
    if (!this.#snapshotSent) {
      // The first snapshot will carry these values.
      this.#pending.clear();
      return;
    }
    const ops = this.#pending.take();
    if (ops.length > 0) this.#send("state.patch", { ops, ts: Date.now() });
  }

  #send(method: string, params: unknown) {
    if (this.#destroyed && method !== "log") return;
    this.#sink.notify(method, params);
  }

  #sendSnapshot() {
    clearTimeout(this.#timer);
    this.#timer = undefined;
    this.#pending.clear();
    this.#snapshotSent = true;
    this.#send("state.snapshot", {
      values: this.#tree.leaves(),
      complete: true,
    });
  }

  #queue(ops: PatchOp[]) {
    if (ops.length === 0 || this.#destroyed) return;
    for (const op of ops) this.#pending.add(op);
    this.#timer ??= setTimeout(() => this.flush(), this.#coalesceMs);
  }

  #log(level: LogLevel, msg: string, fields?: Record<string, unknown>) {
    this.#send("log", fields ? { level, msg, fields } : { level, msg });
  }

  #makeContext(): BaseContext<unknown, Record<string, unknown>> {
    const tree = this.#tree;
    const write =
      (fn: (path: Path, value: unknown) => PatchOp[]) =>
      (pathOrValue: PathLike | Record<string, unknown>, value?: unknown) => {
        if (typeof pathOrValue === "string" || Array.isArray(pathOrValue)) {
          this.#queue(fn(toPath(pathOrValue as PathLike), value));
        } else {
          this.#queue(fn([], pathOrValue));
        }
      };
    const state: StateApi = {
      get: (path) => tree.get(path === undefined ? [] : toPath(path)),
      set: write((p, v) => tree.set(p, v)),
      patch: write((p, v) => tree.patch(p, v)),
      delete: (path) => this.#queue(tree.delete(toPath(path))),
    };
    const log: Logger = {
      trace: (msg, fields) => this.#log("trace", msg, fields),
      debug: (msg, fields) => this.#log("debug", msg, fields),
      info: (msg, fields) => this.#log("info", msg, fields),
      warn: (msg, fields) => this.#log("warn", msg, fields),
      error: (msg, fields) => this.#log("error", msg, fields),
    };
    const def = this.#plugin.definition;
    const init = this.#init;
    return {
      conn: init.conn,
      alias: init.alias,
      mode: init.mode,
      config: withDefaults(def.config, init.config ?? {}),
      secrets: init.secrets ?? {},
      state,
      log,
      signal: this.#abort.signal,
      emit: (event, payload) => {
        if (this.#destroyed) return;
        this.flush(); // keep patches written before the event ahead of it
        this.#send(
          "event.emit",
          payload === undefined
            ? { event, ts: Date.now() }
            : { event, payload, ts: Date.now() },
        );
      },
      status: (status, message, detail) => {
        if (this.#destroyed) return;
        const previous = this.#status;
        this.#status = status;
        if (status === "ok" && previous !== "ok") {
          // Coming up (or back): the snapshot refreshes values the core
          // marked stale, and goes out before `ok` (spec §3).
          this.#sendSnapshot();
        } else {
          this.flush();
        }
        const params: Record<string, unknown> = { status };
        if (message !== undefined) params.message = message;
        if (detail !== undefined) params.detail = detail;
        this.#send("conn.status", params);
      },
    };
  }
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
