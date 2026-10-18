import "@typespec/json-schema";

using TypeSpec.JsonSchema;

/** Actions. Spec §5. */
@jsonSchema
namespace Covalence.Protocol;

/** `action.invoke` params (core → host, req). Result: `ActionResult`. */
model ActionInvoke {
  /** ULID. Idempotency key; matches the journal's `action.requested.call`. */
  call: string;

  /** Flow run ID, for logs. */
  run?: string;

  /** Fencing term. The host rejects any term lower than the highest it has seen. */
  term: uint32;

  action: string;
  params: unknown;

  /** Milliseconds. The host gives up after this long. */
  deadline?: uint32;
}

/** `action.invoke` result. */
model ActionResult {
  result?: unknown;
}
