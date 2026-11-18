import "@typespec/json-schema";
import "./io.tsp";

using TypeSpec.JsonSchema;

/** Plugin loading and the manifest. Spec §3.2 and §4. */
@jsonSchema
namespace Covalence.Protocol;

/** A JSON Schema document. Opaque to the protocol (plugin protocol spec §13.2). */
model JsonSchemaDoc is Record<unknown>;

/** `plugin.load` params (core → host, req). Result: `PluginLoaded`. */
model PluginLoad {
  /** Plugin ID, for example `io.covalence.videohub`. */
  plugin: string;

  version: string;

  /** Path to the bundle, or a blob reference for an Orbital. */
  bundle: string;
}

/** `plugin.load` result. */
model PluginLoaded {
  manifest: Manifest;
}

model Manifest {
  /** Reverse-DNS ID, for example `io.covalence.videohub`. */
  id: string;

  name: string;

  /** Semver. */
  version: string;

  /** Semver range of supported protocol versions, for example `^0.1`. */
  protocol: string;

  sdk: string;
  runtime: "deno";
  license: string;
  authors: string[];
  homepage?: string;
  categories: string[];
  icon?: string;
  config: ConfigDef;
  secrets?: Record<SecretDef>;

  /** Declared, but not yet enforced (plugin protocol spec §13.5). */
  permissions: Permissions;

  connections: ConnectionPolicy;
  actions: ActionDef[];
  events: EventDef[];

  /** State schema (state spec §1.2–1.4). */
  state: Record<unknown>;

  presets?: unknown[];
  surfaces?: Record<unknown>;
  simulator?: SimulatorSupport;

  /** Devices the plugin drives through the core (§7.1). */
  devices?: DeviceMatch[];

  /** Opens network sockets itself: its traffic isn't in the inspector (§7). */
  directNetwork?: boolean;
}

model ConfigDef {
  /** Drives the connection form. */
  schema: JsonSchemaDoc;

  /** Field order, groups, help, conditional visibility. */
  ui?: Record<unknown>;

  /** Config schema version (state spec §7.2). */
  version: uint32;
}

model SecretDef {
  label: string;
  description?: string;
}

model Permissions {
  /** Templates, for example `{config.host}:{config.port}`. */
  net?: string[];

  serial?: boolean;
  udpBroadcast?: boolean;
  env?: string[];
}

/** Policy for standby nodes (state spec §7.4). */
union ConnectionPolicy {
  "multiple",
  "single",
}

model ActionDef {
  id: string;
  name: string;
  description?: string;
  params: JsonSchemaDoc;
  result?: JsonSchemaDoc;

  /** Safe to retry with the same call ID. */
  idempotent?: boolean;

  cancellable?: boolean;
  durationHint?: "instant" | "short" | "long";

  /** Supports "learn current values" for its params. */
  learn?: boolean;

  /** `device` (the default) changes the world outside Covalence; simulate mode suppresses it. `internal` only changes Covalence itself. */
  effect?: "internal" | "device";
}

model EventDef {
  id: string;
  name: string;
  payload?: JsonSchemaDoc;
}

model SimulatorSupport {
  stateLevel: boolean;
  protocolLevel: boolean;
}
