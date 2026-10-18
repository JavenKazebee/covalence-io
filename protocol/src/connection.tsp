import "@typespec/json-schema";

using TypeSpec.JsonSchema;

/**
 * Connection lifecycle. Spec §3.3 and §3.4.
 *
 * The connection ID travels in the envelope's `conn`, not in these params.
 */
@jsonSchema
namespace Covalence.Protocol;

/** `conn.create` params (core → host, req). Result: `Empty`. */
model ConnCreate {
  plugin: string;
  version: string;
  `alias`: string;
  config: Record<unknown>;
  secrets: Record<string>;

  /** Resolved from the manifest templates and the config. Not yet enforced (ADR 0015). */
  permissions: Permissions;

  mode: ConnMode;
  isolate: boolean;
}

union ConnMode {
  "live",
  "simulate",
}

/** `conn.destroy` params (core → host, req). Result: `Empty`. */
model ConnDestroy {
  reason: string;
}

// `conn.resync` (core → host, req) takes and returns `Empty`. The host answers
// by sending a fresh `state.snapshot`.

/** `conn.status` params (host → core, ntf). */
model ConnStatus {
  status: ConnStatusKind;
  message?: string;
  detail?: unknown;
}

union ConnStatusKind {
  "connecting",
  "ok",
  "warning",
  "error",
  "disconnected",
}
