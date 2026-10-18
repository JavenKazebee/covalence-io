import "@typespec/json-schema";

using TypeSpec.JsonSchema;

/** Plugin protocol messages. Spec §2.1. */
@jsonSchema
namespace Covalence.Protocol;

/** A request; the peer must answer with a response carrying the same `id`. */
model Request {
  t: "req";

  /** Unique per direction per link. */
  id: uint32;

  /** Method name, for example `host.hello`. */
  m: string;

  /** Connection this message concerns. Hosts route on it without decoding `p`. */
  conn?: string;

  /** Method parameters. */
  p: unknown;
}

/** A successful response. */
model ResponseOk {
  t: "res";
  id: uint32;
  ok: true;

  /** Method result. */
  r: unknown;
}

/** A failed response. */
model ResponseErr {
  t: "res";
  id: uint32;
  ok: false;
  e: ProtocolError;
}

/** A fire-and-forget notification. */
model Notification {
  t: "ntf";
  m: string;
  conn?: string;
  p: unknown;
}

union Message {
  Request,
  ResponseOk,
  ResponseErr,
  Notification,
}

model ProtocolError {
  code: ErrorCode;
  message: string;
  data?: unknown;
  retryable?: boolean;
}

/** Spec §9. */
union ErrorCode {
  "invalid_request",
  "unknown_method",
  "unsupported_version",
  "invalid_params",
  "not_found",
  "not_connected",
  "permission_denied",
  "action_failed",
  "timeout",
  "cancelled",
  "stale_term",
  "restart_required",
  "overloaded",
  "internal",
}

/** Params or result of a method that carries none. */
model Empty {}
