/**
 * Messages between the host's main thread and a connection worker
 * (structured clone, not the wire protocol).
 *
 * Params pass through as the core sent them; the worker decodes them.
 *
 * @module
 */

import type { ProtocolError } from "@covalence-io/protocol";

/** Methods the main thread asks a worker to run. */
export type WorkerMethod =
  | "conn.create"
  | "conn.destroy"
  | "conn.resync"
  | "action.invoke";

export type ToWorker =
  | { type: "req"; id: number; m: WorkerMethod; p: unknown }
  /** The call's deadline passed: abort it (its result is no longer wanted). */
  | { type: "cancel"; call: string };

export type FromWorker =
  | { type: "res"; id: number; ok: true; r: unknown }
  | { type: "res"; id: number; ok: false; e: ProtocolError }
  /** A host → core notification for this worker's connection. */
  | { type: "ntf"; m: string; p: unknown };

/** `conn.create` params as the worker gets them: the core's plus what the host resolved. */
export interface WorkerCreate {
  /** URL of the plugin bundle. */
  bundle: string;
  conn: string;
  params: {
    alias: string;
    mode: "live" | "simulate";
    config: Record<string, unknown>;
    secrets: Record<string, string | undefined>;
  };
}
