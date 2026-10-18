import "@typespec/json-schema";

using TypeSpec.JsonSchema;

/** Session lifecycle. Spec §3.1 and §2 (keepalive). */
@jsonSchema
namespace Covalence.Protocol;

/** `host.hello` params (host → core, req). Result: `HostWelcome`. */
model HostHello {
  protocol: ProtocolRange;
  host: HostInfo;

  /** Orbital only. */
  node?: NodeInfo;

  capabilities: Capability[];
}

model ProtocolRange {
  /** Lowest protocol version the host speaks, for example `0.1.0`. */
  min: string;

  /** Highest protocol version the host speaks, for example `0.1.x`. */
  max: string;
}

model HostInfo {
  kind: HostKind;
  version: string;

  /** For example `deno 2.9.5`. */
  runtime?: string;
}

union HostKind {
  "deno-shared",
  "deno-isolated",
  "orbital",
  "wasm",
  "external",
}

model NodeInfo {
  id: string;
  name: string;
  publicKey: string;
}

union Capability {
  "workers",
  "permissions",
  "surfaces",
  "transports",
  "simulate",
  "computerControl",
}

/** `host.hello` result. */
model HostWelcome {
  /** The protocol version chosen for this session. */
  protocol: string;

  session: string;

  /** Leadership term (ADR 0008). */
  term: uint32;

  coreId: string;
}

// `ping` (either direction, req) takes and returns `Empty`. A peer that
// misses three consecutive pings, sent every 2 s, is treated as dead.
