import "@typespec/json-schema";

using TypeSpec.JsonSchema;

/**
 * Host transports and devices: I/O the core performs for a connection.
 * Spec §7 and §7.1 (ADR 0025).
 *
 * The connection is the envelope's `conn`. Bytes are `bin` in MessagePack,
 * and an array of byte values in JSON.
 */
@jsonSchema
namespace Covalence.Protocol;

/** Raw bytes: `bin` in MessagePack, an array of byte values in JSON. */
model Bytes is uint8[];

union IoKind {
  "tcp",
  "udp",
  "ws",
  "serial",
  "hid",
  "midi",
}

union Framing {
  "raw",
  "line",
  "length",
}

model Backoff {
  initialMs?: safeint;
  maxMs?: safeint;
}

model IoOptions {
  /** Reopen after the far end closes or the device returns. */
  reconnect?: boolean;

  backoff?: Backoff;
  framing?: Framing;

  /** `serial` only. */
  baud?: safeint;
}

/** `io.open` params (host → core, req). Result: `IoOpened`. */
model IoOpen {
  kind: IoKind;

  /** `host:port` for `tcp`; a device ID (§7.1) for `serial`, `hid` and `midi`. */
  target: string;

  options?: IoOptions;
}

model IoOpened {
  handle: string;
}

/** `io.write` params (host → core, ntf). For `hid`, an output report whose first byte is the report ID. */
model IoWrite {
  handle: string;
  data: Bytes;
}

/** `io.data` params (core → host, ntf). For `hid`, an input report. */
model IoData {
  handle: string;
  data: Bytes;
  /** (M7) When the core read it: milliseconds since the Unix epoch, on the core's clock. */
  ts?: float64;
}

/** `io.feature` params (host → core, req), `hid` only. Result: `IoFeatureResult`. */
model IoFeature {
  handle: string;
  `op`: "get" | "send";

  /** `get`: which report, and how many bytes to read. */
  reportId?: safeint;

  length?: safeint;

  /** `send`: the report, its ID first. */
  data?: Bytes;
}

/** `get` gives `data`; `send` gives nothing. */
model IoFeatureResult {
  data?: Bytes;
}

union IoStateKind {
  "open",
  "closed",
  "reconnecting",
  "error",
}

/**
 * `io.state` params (core → host, ntf). A device handle closes with
 * `error: "unplugged"`, and opens again when the same device returns.
 */
model IoState {
  handle: string;
  state: IoStateKind;
  error?: string;
}

/** `io.close` params (host → core, ntf). */
model IoClose {
  handle: string;
}

union DeviceKind {
  "hid",
  "serial",
  "midi",
}

/** A manifest's `devices` entry: which devices the plugin drives. */
model DeviceMatch {
  kind: DeviceKind;
  vendorId?: safeint;
  productId?: safeint | safeint[];

  /** `hid` only. */
  usagePage?: safeint;

  /** A glob over the product or MIDI port name. */
  name?: string;

  /** Create a connection without asking (surfaces). */
  autoCreate?: boolean;
}

/** A device the core found. */
model DeviceInfo {
  /** Stable: from kind, vendor, product and serial number where there is one, else the port path. */
  id: string;

  kind: DeviceKind;
  vendorId?: safeint;
  productId?: safeint;
  serial?: string;
  manufacturer?: string;
  product?: string;

  /** OS path or port name, for display. */
  path: string;

  usagePage?: safeint;
  usage?: safeint;
}

/** `device.list` params (host → core, req): devices this plugin's `devices` match. Result: `DeviceInfoList`. */
model DeviceList {
  kind?: DeviceKind;
}

model DeviceInfoList is DeviceInfo[];
