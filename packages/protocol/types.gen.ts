// Generated from protocol/*.tsp by protocol/scripts/gen-ts.mjs. Do not edit.

export type CovalenceProtocol =
  | Request
  | ResponseOk
  | ResponseErr
  | ProtocolError
  | ErrorCode
  | Notification
  | Empty
  | HostHello
  | ProtocolRange
  | HostInfo
  | HostKind
  | NodeInfo
  | Capability
  | HostWelcome
  | JsonSchemaDoc
  | PluginLoad
  | PluginLoaded
  | Manifest
  | ConfigDef
  | SecretDef
  | Permissions
  | ConnectionPolicy
  | ActionDef
  | EventDef
  | SimulatorSupport
  | ConnCreate
  | ConnMode
  | ConnDestroy
  | ConnStatus
  | ConnStatusKind
  | PathValue
  | PathSegment
  | StateSnapshot
  | StatePatch
  | PatchOp
  | PathDelete
  | EventEmit
  | Log
  | LogLevel
  | ActionInvoke
  | ActionResult
  | Message;
/**
 * Spec §9.
 */
export type ErrorCode =
  | "invalid_request"
  | "unknown_method"
  | "unsupported_version"
  | "invalid_params"
  | "not_found"
  | "not_connected"
  | "permission_denied"
  | "action_failed"
  | "timeout"
  | "cancelled"
  | "stale_term"
  | "restart_required"
  | "overloaded"
  | "internal";
export type HostKind = "deno-shared" | "deno-isolated" | "orbital" | "wasm" | "external";
export type Capability = "workers" | "permissions" | "surfaces" | "transports" | "simulate" | "computerControl";
/**
 * Policy for standby nodes (ADR 0008).
 */
export type ConnectionPolicy = "multiple" | "single";
export type ConnMode = "live" | "simulate";
export type ConnStatusKind = "connecting" | "ok" | "warning" | "error" | "disconnected";
/**
 * A path segment: a key, or an integer collection key.
 */
export type PathSegment = string | number;
export type PatchOp = PathValue | PathDelete;
export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";
export type Message = Request | ResponseOk | ResponseErr | Notification;

/**
 * A request; the peer must answer with a response carrying the same `id`.
 */
export interface Request {
  t: "req";
  /**
   * Unique per direction per link.
   */
  id: number;
  /**
   * Method name, for example `host.hello`.
   */
  m: string;
  /**
   * Connection this message concerns. Hosts route on it without decoding `p`.
   */
  conn?: string;
  /**
   * Method parameters.
   */
  p: {
    [k: string]: unknown | undefined;
  };
}
/**
 * A successful response.
 */
export interface ResponseOk {
  t: "res";
  id: number;
  ok: true;
  /**
   * Method result.
   */
  r: {
    [k: string]: unknown | undefined;
  };
}
/**
 * A failed response.
 */
export interface ResponseErr {
  t: "res";
  id: number;
  ok: false;
  e: ProtocolError;
}
export interface ProtocolError {
  code: ErrorCode;
  message: string;
  data?: unknown;
  retryable?: boolean;
}
/**
 * A fire-and-forget notification.
 */
export interface Notification {
  t: "ntf";
  m: string;
  conn?: string;
  p: unknown;
}
/**
 * Params or result of a method that carries none.
 */
export interface Empty {}
/**
 * `host.hello` params (host → core, req). Result: `HostWelcome`.
 */
export interface HostHello {
  protocol: ProtocolRange;
  host: HostInfo;
  node?: NodeInfo;
  capabilities: Capability[];
}
export interface ProtocolRange {
  /**
   * Lowest protocol version the host speaks, for example `0.1.0`.
   */
  min: string;
  /**
   * Highest protocol version the host speaks, for example `0.1.x`.
   */
  max: string;
}
export interface HostInfo {
  kind: HostKind;
  version: string;
  /**
   * For example `deno 2.9.5`.
   */
  runtime?: string;
}
export interface NodeInfo {
  id: string;
  name: string;
  publicKey: string;
}
/**
 * `host.hello` result.
 */
export interface HostWelcome {
  /**
   * The protocol version chosen for this session.
   */
  protocol: string;
  session: string;
  /**
   * Leadership term (ADR 0008).
   */
  term: number;
  coreId: string;
}
/**
 * A JSON Schema document. Opaque to the protocol (ADR 0016).
 */
export interface JsonSchemaDoc {
  [k: string]: unknown | undefined;
}
/**
 * `plugin.load` params (core → host, req). Result: `PluginLoaded`.
 */
export interface PluginLoad {
  /**
   * Plugin ID, for example `io.covalence.videohub`.
   */
  plugin: string;
  version: string;
  /**
   * Path to the bundle, or a blob reference for an Orbital.
   */
  bundle: string;
}
/**
 * `plugin.load` result.
 */
export interface PluginLoaded {
  manifest: Manifest;
}
export interface Manifest {
  /**
   * Reverse-DNS ID, for example `io.covalence.videohub`.
   */
  id: string;
  name: string;
  /**
   * Semver.
   */
  version: string;
  /**
   * Semver range of supported protocol versions, for example `^0.1`.
   */
  protocol: string;
  sdk: string;
  runtime: "deno";
  license: string;
  authors: string[];
  homepage?: string;
  categories: string[];
  icon?: string;
  config: ConfigDef;
  secrets?: {
    [k: string]: SecretDef | undefined;
  };
  permissions: Permissions;
  connections: ConnectionPolicy;
  actions: ActionDef[];
  events: EventDef[];
  /**
   * State schema (state spec §1.2–1.4).
   */
  state: {
    [k: string]: unknown | undefined;
  };
  presets?: unknown[];
  surfaces?: {
    [k: string]: unknown | undefined;
  };
  simulator?: SimulatorSupport;
}
export interface ConfigDef {
  schema: JsonSchemaDoc;
  /**
   * Field order, groups, help, conditional visibility.
   */
  ui?: {
    [k: string]: unknown | undefined;
  };
  /**
   * Config schema version (ADR 0006).
   */
  version: number;
}
export interface SecretDef {
  label: string;
  description?: string;
}
export interface Permissions {
  /**
   * Templates, for example `{config.host}:{config.port}`.
   */
  net?: string[];
  serial?: boolean;
  udpBroadcast?: boolean;
  env?: string[];
}
export interface ActionDef {
  id: string;
  name: string;
  description?: string;
  params: JsonSchemaDoc;
  result?: JsonSchemaDoc;
  /**
   * Safe to retry with the same call ID.
   */
  idempotent?: boolean;
  cancellable?: boolean;
  durationHint?: "instant" | "short" | "long";
  /**
   * Supports "learn current values" for its params.
   */
  learn?: boolean;
}
export interface EventDef {
  id: string;
  name: string;
  payload?: JsonSchemaDoc;
}
export interface SimulatorSupport {
  stateLevel: boolean;
  protocolLevel: boolean;
}
/**
 * `conn.create` params (core → host, req). Result: `Empty`.
 */
export interface ConnCreate {
  plugin: string;
  version: string;
  alias: string;
  config: {
    [k: string]: unknown | undefined;
  };
  secrets: {
    [k: string]: string | undefined;
  };
  permissions: Permissions;
  mode: ConnMode;
  isolate: boolean;
}
/**
 * `conn.destroy` params (core → host, req). Result: `Empty`.
 */
export interface ConnDestroy {
  reason: string;
}
/**
 * `conn.status` params (host → core, ntf).
 */
export interface ConnStatus {
  status: ConnStatusKind;
  message?: string;
  detail?: unknown;
}
/**
 * A path and its value.
 */
export interface PathValue {
  p: PathSegment[];
  v: unknown;
}
/**
 * `state.snapshot` params (ntf). Replaces the connection's whole subtree.
 */
export interface StateSnapshot {
  values: PathValue[];
  complete: true;
}
/**
 * `state.patch` params (ntf).
 */
export interface StatePatch {
  ops: PatchOp[];
  /**
   * The `action.invoke` call that produced this change, if any. Unset for
   * changes from device traffic outside an action.
   */
  call?: string;
  /**
   * Unix epoch milliseconds.
   */
  ts?: number;
}
export interface PathDelete {
  p: PathSegment[];
  del: true;
}
/**
 * `event.emit` params (ntf).
 */
export interface EventEmit {
  event: string;
  payload?: unknown;
  /**
   * The `action.invoke` call that produced this event, if any.
   */
  call?: string;
  /**
   * Unix epoch milliseconds.
   */
  ts?: number;
}
/**
 * `log` params (ntf). The envelope's `conn` is optional here.
 */
export interface Log {
  level: LogLevel;
  msg: string;
  fields?: {
    [k: string]: unknown | undefined;
  };
}
/**
 * `action.invoke` params (core → host, req). Result: `ActionResult`.
 */
export interface ActionInvoke {
  /**
   * ULID. Idempotency key; matches the journal's `action.requested.call`.
   */
  call: string;
  /**
   * Flow run ID, for logs.
   */
  run?: string;
  /**
   * Fencing term. The host rejects any term lower than the highest it has seen.
   */
  term: number;
  action: string;
  params: unknown;
  /**
   * Milliseconds. The host gives up after this long.
   */
  deadline?: number;
}
/**
 * `action.invoke` result.
 */
export interface ActionResult {
  result?: unknown;
}
