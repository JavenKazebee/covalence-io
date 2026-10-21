// The HTTP API, as listed in crates/covalence-server/src/lib.rs.

import type { HostStatus, JournalEntry } from "./live/protocol";

/** A JSON Schema, as plugin manifests write them. Only what the forms read is typed. */
export interface JsonSchema {
  type?: "object" | "string" | "integer" | "number" | "boolean" | "array";
  title?: string;
  description?: string;
  default?: unknown;
  enum?: unknown[];
  minimum?: number;
  maximum?: number;
  minLength?: number;
  format?: string;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  /** Choices taken from one of the connection's state collections, e.g. `{from: "inputs"}`. */
  "x-choices"?: { from: string };
}

export interface ManifestAction {
  id: string;
  name: string;
  description?: string;
  params: JsonSchema;
}

/** A state collection in the manifest: items keyed by `key`, labelled by `label`. */
export interface ManifestCollection {
  collection: { key: string; label?: string };
  item: Record<string, JsonSchema>;
}

export interface Manifest {
  id: string;
  name: string;
  version: string;
  config: { version: number; schema: JsonSchema };
  actions: ManifestAction[];
  state: Record<string, JsonSchema | ManifestCollection>;
}

export interface Plugin {
  id: string;
  version: string;
  bundle: string;
  /** Null until the plugin host has loaded the plugin. */
  manifest: Manifest | null;
}

export type ConnMode = "live" | "simulate";

export interface Connection {
  id: string;
  plugin: string;
  version: string;
  alias: string;
  config: Record<string, unknown>;
  mode: ConnMode;
  permissions: Record<string, unknown>;
}

export interface NewConnection {
  plugin: string;
  alias: string;
  config: Record<string, unknown>;
  mode: ConnMode;
}

export interface ActionOutcome {
  call: string;
  /** Seq of the `action.requested` entry. */
  requested: number;
  verdict: "dispatched" | { suppressed: string };
  /** Seq of the `action.result` entry, once there is one. */
  resultSeq: number | null;
  ok?: boolean;
  result?: unknown;
  error?: { code: string; message: string };
}

export interface Health {
  host: HostStatus;
  journal: { lastSeq: number; term: number };
}

/** A non-2xx answer. `message` is the server's `error` field when it sent one. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

async function call<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const json: unknown = text === "" ? null : JSON.parse(text);
  if (!response.ok) {
    const error = (json as { error?: unknown } | null)?.error;
    throw new ApiError(response.status, typeof error === "string" ? error : `${method} ${path}: ${response.status}`);
  }
  return json as T;
}

export const api = {
  health: () => call<Health>("GET", "/health"),
  plugins: () => call<Plugin[]>("GET", "/plugins"),
  connections: () => call<Connection[]>("GET", "/connections"),
  createConnection: (req: NewConnection) => call<Connection>("POST", "/connections", req),
  requestAction: (action: string, params: Record<string, unknown>) =>
    call<ActionOutcome>("POST", "/actions", { action, params }),
  journal: (from: number, limit: number) =>
    call<JournalEntry[]>("GET", `/journal?from=${from}&limit=${limit}`),
  why: (seq: number) => call<JournalEntry[]>("GET", `/journal/${seq}/why`),
};
