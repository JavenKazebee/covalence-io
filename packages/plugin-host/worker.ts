/**
 * A connection worker: runs one connection of one plugin (ADR 0003) with the
 * SDK's connection runtime. Permissions are inherited from the host process
 * (ADR 0015: no per-worker narrowing yet).
 *
 * @module
 */

import type { ActionInvoke, ProtocolError } from "@covalence-io/protocol";
import { ActionError, type Plugin } from "@covalence-io/sdk";
import { ConnectionRuntime } from "@covalence-io/sdk/runtime";
import type { FromWorker, ToWorker, WorkerCreate } from "./worker_messages.ts";

// The worker global scope, typed here so this file checks with the rest of
// the workspace (which uses the window lib).
const scope = globalThis as unknown as {
  postMessage(msg: FromWorker): void;
  onmessage: ((e: MessageEvent<ToWorker>) => void) | null;
  addEventListener(
    type: "unhandledrejection",
    fn: (e: PromiseRejectionEvent) => void,
  ): void;
};
const post = (msg: FromWorker) => scope.postMessage(msg);

// Stdout carries the protocol, so plugin console output becomes `log`
// notifications instead.
const levels = {
  trace: "trace",
  debug: "debug",
  log: "info",
  info: "info",
  warn: "warn",
  error: "error",
} as const;
for (const [method, level] of Object.entries(levels)) {
  (console as unknown as Record<string, unknown>)[method] = (
    ...args: unknown[]
  ) =>
    post({
      type: "ntf",
      m: "log",
      p: {
        level,
        msg: args.map((a) => typeof a === "string" ? a : Deno.inspect(a))
          .join(" "),
      },
    });
}

scope.addEventListener("unhandledrejection", (e) => {
  e.preventDefault(); // a plugin bug shouldn't kill the connection
  post({
    type: "ntf",
    m: "log",
    p: { level: "error", msg: `unhandled rejection: ${message(e.reason)}` },
  });
});

// Requests that arrive while conn.create is still importing the bundle wait
// for it.
const created = Promise.withResolvers<ConnectionRuntime>();
const calls = new Map<string, AbortController>();

scope.onmessage = async (event: MessageEvent<ToWorker>) => {
  const msg = event.data;
  if (msg.type === "cancel") {
    calls.get(msg.call)?.abort(new Error("deadline passed"));
    return;
  }
  try {
    const r = await handle(msg.m, msg.p);
    post({ type: "res", id: msg.id, ok: true, r });
  } catch (err) {
    post({ type: "res", id: msg.id, ok: false, e: toProtocolError(err) });
  }
};

async function handle(method: string, params: unknown): Promise<unknown> {
  switch (method) {
    case "conn.create": {
      const { bundle, conn, params: p } = params as WorkerCreate;
      const plugin = await loadPlugin(bundle);
      const runtime = new ConnectionRuntime(plugin, { conn, ...p }, {
        notify: (m, p) => post({ type: "ntf", m, p }),
      });
      created.resolve(runtime);
      // Reply first; status and the snapshot follow as notifications.
      setTimeout(() => runtime.start(), 0);
      return {};
    }
    case "action.invoke": {
      const invoke = params as ActionInvoke;
      const controller = new AbortController();
      calls.set(invoke.call, controller);
      try {
        return await (await created.promise).invoke(invoke, controller.signal);
      } finally {
        calls.delete(invoke.call);
      }
    }
    case "conn.resync":
      (await created.promise).resync();
      return {};
    case "conn.destroy":
      await (await created.promise).destroy(
        (params as { reason?: string })?.reason ?? "",
      );
      return {};
    default:
      throw new ActionError("unknown_method", `unknown method ${method}`);
  }
}

async function loadPlugin(bundle: string): Promise<Plugin> {
  const mod = await import(bundle);
  const plugin = mod.default as Plugin | undefined;
  if (!plugin?.manifest || !plugin.definition) {
    throw new Error(`${bundle} has no definePlugin() default export`);
  }
  return plugin;
}

function toProtocolError(err: unknown): ProtocolError {
  if (err instanceof ActionError) {
    const e: ProtocolError = { code: err.code, message: err.message };
    if (err.data !== undefined) e.data = err.data;
    if (err.retryable !== undefined) e.retryable = err.retryable;
    return e;
  }
  return { code: "internal", message: message(err) };
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
