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
  | DeviceMatch
  | DeviceKind
  | Bytes
  | Backoff
  | IoOptions
  | Framing
  | IoOpen
  | IoKind
  | IoOpened
  | IoWrite
  | IoData
  | IoFeature
  | IoFeatureResult
  | IoState
  | IoStateKind
  | IoClose
  | DeviceInfo
  | DeviceList
  | DeviceInfoList
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
  | Placement
  | MomentaryInput
  | RelativeInput
  | AbsoluteInput
  | TouchInput
  | ImageOutput
  | LedOutput
  | LevelOutput
  | TextOutput
  | ControlDef
  | InputCap
  | OutputCap
  | SurfaceOutputs
  | SurfaceAttach
  | SurfaceDetach
  | SurfaceInput
  | InputKind
  | SurfaceDraw
  | ImageFormat
  | SurfaceLevel
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
  | "busy"
  | "restart_required"
  | "overloaded"
  | "internal";
export type HostKind = "deno-shared" | "deno-isolated" | "orbital" | "wasm" | "external";
export type Capability = "workers" | "permissions" | "surfaces" | "transports" | "simulate" | "computerControl";
/**
 * Policy for standby nodes (ADR 0008).
 */
export type ConnectionPolicy = "multiple" | "single";
export type DeviceKind = "hid" | "serial" | "midi";
/**
 * Raw bytes: `bin` in MessagePack, an array of byte values in JSON.
 */
export type Bytes = Uint8Array | number[];
export type Framing = "raw" | "line" | "length";
export type IoKind = "tcp" | "udp" | "ws" | "serial" | "hid" | "midi";
export type IoStateKind = "open" | "closed" | "reconnecting" | "error";
export type DeviceInfoList = DeviceInfo[];
export type ConnMode = "live" | "simulate";
export type ConnStatusKind = "connecting" | "ok" | "warning" | "error" | "disconnected";
/**
 * A path segment: a key, or an integer collection key.
 */
export type PathSegment = string | number;
export type PatchOp = PathValue | PathDelete;
export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";
/**
 * What a control can send.
 */
export type InputCap = MomentaryInput | RelativeInput | AbsoluteInput | TouchInput;
/**
 * What a control can show.
 */
export type OutputCap = ImageOutput | LedOutput | LevelOutput | TextOutput;
export type InputKind = "down" | "up" | "turn" | "touch" | "value";
/**
 * (M7) `rgb`: raw 8-bit RGB, rows top to bottom, for devices whose driver packs images itself.
 */
export type ImageFormat = "jpeg" | "bmp" | "png" | "rgb565" | "rgb";
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
  /**
   * Devices the plugin drives through the core (§7.1).
   */
  devices?: DeviceMatch[];
  /**
   * Opens network sockets itself: its traffic isn't in the inspector (§7).
   */
  directNetwork?: boolean;
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
  /**
   * `device` (the default) changes the world outside Covalence; simulate mode suppresses it. `internal` only changes Covalence itself.
   */
  effect?: "internal" | "device";
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
 * A manifest's `devices` entry: which devices the plugin drives.
 */
export interface DeviceMatch {
  kind: DeviceKind;
  vendorId?: number;
  productId?: number | number[];
  /**
   * `hid` only.
   */
  usagePage?: number;
  /**
   * A glob over the product or MIDI port name.
   */
  name?: string;
  /**
   * Create a connection without asking (surfaces).
   */
  autoCreate?: boolean;
}
export interface Backoff {
  initialMs?: number;
  maxMs?: number;
}
export interface IoOptions {
  /**
   * Reopen after the far end closes or the device returns.
   */
  reconnect?: boolean;
  backoff?: Backoff;
  framing?: Framing;
  /**
   * `serial` only.
   */
  baud?: number;
}
/**
 * `io.open` params (host → core, req). Result: `IoOpened`.
 */
export interface IoOpen {
  kind: IoKind;
  /**
   * `host:port` for `tcp`; a device ID (§7.1) for `serial`, `hid` and `midi`.
   */
  target: string;
  options?: IoOptions;
}
export interface IoOpened {
  handle: string;
}
/**
 * `io.write` params (host → core, ntf). For `hid`, an output report whose first byte is the report ID.
 */
export interface IoWrite {
  handle: string;
  data: Bytes;
}
/**
 * `io.data` params (core → host, ntf). For `hid`, an input report.
 */
export interface IoData {
  handle: string;
  data: Bytes;
  /**
   * (M7) When the core read it: milliseconds since the Unix epoch, on the core's clock.
   */
  ts?: number;
}
/**
 * `io.feature` params (host → core, req), `hid` only. Result: `IoFeatureResult`.
 */
export interface IoFeature {
  handle: string;
  op: "get" | "send";
  /**
   * `get`: which report, and how many bytes to read.
   */
  reportId?: number;
  length?: number;
  data?: Bytes;
}
/**
 * `get` gives `data`; `send` gives nothing.
 */
export interface IoFeatureResult {
  data?: Bytes;
}
/**
 * `io.state` params (core → host, ntf). A device handle closes with
 * `error: "unplugged"`, and opens again when the same device returns.
 */
export interface IoState {
  handle: string;
  state: IoStateKind;
  error?: string;
}
/**
 * `io.close` params (host → core, ntf).
 */
export interface IoClose {
  handle: string;
}
/**
 * A device the core found.
 */
export interface DeviceInfo {
  /**
   * Stable: from kind, vendor, product and serial number where there is one, else the port path.
   */
  id: string;
  kind: DeviceKind;
  vendorId?: number;
  productId?: number;
  serial?: string;
  manufacturer?: string;
  product?: string;
  /**
   * OS path or port name, for display.
   */
  path: string;
  usagePage?: number;
  usage?: number;
}
/**
 * `device.list` params (host → core, req): devices this plugin's `devices` match. Result: `DeviceInfoList`.
 */
export interface DeviceList {
  kind?: DeviceKind;
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
   * The node the event happened at, relative to the connection root (`["outputs", 3]`). Absent, it's the root.
   */
  at?: PathSegment[];
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
/**
 * Where a control is, in grid cells.
 */
export interface Placement {
  x: number;
  y: number;
  w?: number;
  h?: number;
}
export interface MomentaryInput {
  kind: "momentary";
  velocity?: boolean;
}
export interface RelativeInput {
  kind: "relative";
}
export interface AbsoluteInput {
  kind: "absolute";
  axes?: 1 | 2;
}
export interface TouchInput {
  kind: "touch";
}
export interface ImageOutput {
  kind: "image";
  w: number;
  h: number;
  format: string;
  rotation?: 0 | 90 | 180 | 270;
}
export interface LedOutput {
  kind: "led";
  rgb?: boolean;
}
export interface LevelOutput {
  kind: "level";
  motor?: boolean;
}
export interface TextOutput {
  kind: "text";
  chars?: number;
  lines?: number;
}
/**
 * One control on a surface.
 */
export interface ControlDef {
  /**
   * Unique on the surface: `k0`, `dial1`, `pad.36`.
   */
  id: string;
  label?: string;
  at: Placement;
  inputs?: InputCap[];
  outputs?: OutputCap[];
}
export interface SurfaceOutputs {
  brightness?: boolean;
}
/**
 * `surface.attach` params (host → core, ntf).
 */
export interface SurfaceAttach {
  surface: string;
  model: string;
  serial?: string;
  name: string;
  controls: ControlDef[];
  outputs?: SurfaceOutputs;
}
/**
 * `surface.detach` params (host → core, ntf).
 */
export interface SurfaceDetach {
  surface: string;
}
/**
 * `surface.input` params (host → core, ntf).
 */
export interface SurfaceInput {
  surface: string;
  control: string;
  kind: InputKind;
  /**
   * Velocity for `down`, detents for `turn`, 0 to 1 (or `[x, y]`) for `value`, 1 or 0 for `touch`.
   */
  value?: number | number[];
  /**
   * The host's timestamp, in milliseconds on its own clock.
   */
  ts: number;
  echo?: boolean;
}
/**
 * `surface.draw` params (core → host, ntf), for `image` outputs.
 */
export interface SurfaceDraw {
  surface: string;
  control: string;
  image: Bytes;
  format: ImageFormat;
}
/**
 * `surface.level` params (core → host, ntf), for `level` outputs: motor faders, encoder rings, meters.
 */
export interface SurfaceLevel {
  surface: string;
  control: string;
  /**
   * 0 to 1.
   */
  value: number;
}
