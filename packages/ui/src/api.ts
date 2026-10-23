// The HTTP API, as listed in crates/covalence-server/src/lib.rs.

import type { ControlBinding, Preview, RuntimeProblem, SavedBinding } from "./bindings/model";
import type { Evaluated, FireResult, Flow, Problem, RunInfo, Saved } from "./flows/model";
import type { HostStatus, JournalEntry } from "./live/protocol";
import type { ControlDef, Layer, SavedLayer, SavedSurface, Surface } from "./surfaces/model";

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

export interface ManifestEvent {
  id: string;
  name: string;
  payload: JsonSchema;
}

export interface Manifest {
  id: string;
  name: string;
  version: string;
  config: { version: number; schema: JsonSchema };
  actions: ManifestAction[];
  events?: ManifestEvent[];
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

/**
 * A non-2xx answer. `message` is the server's `error` field when it sent one;
 * `body` is the whole JSON answer, e.g. a flow's `422 {errors, warnings}`.
 */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
    readonly body: unknown = null,
  ) {
    super(message);
  }

  /** The validation problems of a `422`, if this is one. */
  get problems(): { errors: Problem[]; warnings: Problem[] } | null {
    if (this.status !== 422) return null;
    const b = this.body as { errors?: Problem[]; warnings?: Problem[] } | null;
    return { errors: b?.errors ?? [], warnings: b?.warnings ?? [] };
  }
}

async function call<T>(method: "GET" | "POST" | "PUT" | "DELETE", path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const json: unknown = text === "" ? null : JSON.parse(text);
  if (!response.ok) {
    const error = (json as { error?: unknown } | null)?.error;
    throw new ApiError(response.status, typeof error === "string" ? error : `${method} ${path}: ${response.status}`, json);
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
  flows: () => call<Flow[]>("GET", "/flows"),
  createFlow: (flow: Flow) => call<Saved>("POST", "/flows", flow),
  updateFlow: (flow: Flow) => call<Saved>("PUT", `/flows/${encodeURIComponent(flow.id)}`, flow),
  deleteFlow: (id: string) => call<null>("DELETE", `/flows/${encodeURIComponent(id)}`),
  runFlow: (id: string) => call<FireResult>("POST", `/flows/${encodeURIComponent(id)}/run`, {}),
  runs: (flow: string) => call<RunInfo[]>("GET", `/runs?flow=${encodeURIComponent(flow)}`),
  cancelRun: (run: string) =>
    call<{ run: string; status: RunInfo["status"] }>("POST", `/runs/${encodeURIComponent(run)}/cancel`),
  evaluate: (source: string, trigger?: unknown) =>
    call<Evaluated>("POST", "/expressions/evaluate", trigger === undefined ? { source } : { source, trigger }),
  surfaces: () => call<Surface[]>("GET", "/surfaces"),
  createSurface: (surface: Surface) => call<SavedSurface>("POST", "/surfaces", surface),
  updateSurface: (surface: Surface) =>
    call<SavedSurface>("PUT", `/surfaces/${encodeURIComponent(surface.id)}`, surface),
  deleteSurface: (id: string) => call<null>("DELETE", `/surfaces/${encodeURIComponent(id)}`),
  /** Navigates the surface's `main` group. Answers once the surface has been sent its keys. */
  navigate: (surface: string, layer: string | null) =>
    call<{ surface: string; group: string; layer: string | null }>(
      "POST",
      `/surfaces/${encodeURIComponent(surface)}/current`,
      { layer },
    ),
  /** The controls the surface last attached with; 404 if it hasn't attached since the server started. */
  surfaceControls: (surface: string) => call<ControlDef[]>("GET", `/surfaces/${encodeURIComponent(surface)}/controls`),
  layers: () => call<Layer[]>("GET", "/layers"),
  createLayer: (layer: Layer) => call<SavedLayer>("POST", "/layers", layer),
  updateLayer: (layer: Layer) => call<SavedLayer>("PUT", `/layers/${encodeURIComponent(layer.id)}`, layer),
  deleteLayer: (id: string) => call<null>("DELETE", `/layers/${encodeURIComponent(id)}`),
  bindings: (layer: string) => call<ControlBinding[]>("GET", `/control-bindings?layer=${encodeURIComponent(layer)}`),
  createBinding: (binding: ControlBinding) => call<SavedBinding>("POST", "/control-bindings", binding),
  updateBinding: (binding: ControlBinding) =>
    call<SavedBinding>("PUT", `/control-bindings/${encodeURIComponent(binding.id)}`, binding),
  deleteBinding: (id: string) => call<null>("DELETE", `/control-bindings/${encodeURIComponent(id)}`),
  bindingProblems: () => call<RuntimeProblem[]>("GET", "/control-bindings/problems"),
  previewBinding: (binding: ControlBinding, size?: number) =>
    call<Preview>("POST", "/control-bindings/preview", size === undefined ? { binding } : { binding, size }),
};
