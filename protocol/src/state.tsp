import "@typespec/json-schema";

using TypeSpec.JsonSchema;

/**
 * State, events and logs (host → core). Spec §6.
 *
 * Paths are segment arrays relative to the connection root (ADR 0017).
 */
@jsonSchema
namespace Covalence.Protocol;

/** A path segment: a key, or an integer collection key. */
union PathSegment {
  string,
  safeint,
}

/** A path and its value. */
model PathValue {
  p: PathSegment[];
  v: unknown;
}

/** `state.snapshot` params (ntf). Replaces the connection's whole subtree. */
model StateSnapshot {
  values: PathValue[];
  complete: true;
}

/** `state.patch` params (ntf). */
model StatePatch {
  ops: PatchOp[];

  /**
   * The `action.invoke` call that produced this change, if any. Unset for
   * changes from device traffic outside an action.
   */
  call?: string;

  /** Unix epoch milliseconds. */
  ts?: float64;
}

union PatchOp {
  PathValue,
  PathDelete,
}

model PathDelete {
  p: PathSegment[];
  del: true;
}

/** `event.emit` params (ntf). */
model EventEmit {
  event: string;
  payload?: unknown;

  /** The `action.invoke` call that produced this event, if any. */
  call?: string;

  /** Unix epoch milliseconds. */
  ts?: float64;
}

/** `log` params (ntf). The envelope's `conn` is optional here. */
model Log {
  level: LogLevel;
  msg: string;
  fields?: Record<unknown>;
}

union LogLevel {
  "trace",
  "debug",
  "info",
  "warn",
  "error",
}
