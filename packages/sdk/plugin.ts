/**
 * `definePlugin()`: declares a plugin's manifest and its runtime hooks in one
 * place (spec §4 and §11).
 *
 * @module
 */

import {
  type ActionDef,
  type ConnectionPolicy,
  type ConnMode,
  type ConnStatusKind,
  type ErrorCode,
  type EventDef,
  type JsonSchemaDoc,
  type Manifest,
  type Permissions,
  PROTOCOL_VERSION,
  type SecretDef,
} from "@covalence-io/protocol";
import type { PathLike } from "./paths.ts";
import type { Schema } from "./schema.ts";

/** Version of this SDK, recorded in the manifest. */
export const SDK_VERSION = "0.1.0";

type Fields = Record<string, unknown>;

export interface Logger {
  trace(msg: string, fields?: Fields): void;
  debug(msg: string, fields?: Fields): void;
  info(msg: string, fields?: Fields): void;
  warn(msg: string, fields?: Fields): void;
  error(msg: string, fields?: Fields): void;
}

/**
 * The connection's state subtree. Paths are relative to the connection root,
 * in either form: `"outputs[3].source"` or `["outputs", 3, "source"]`.
 */
export interface StateApi {
  /** Reads a value (the whole tree with no path). */
  get(path?: PathLike): unknown;
  /** Replaces the whole tree. */
  set(value: Record<string, unknown>): void;
  /** Replaces the value at `path`; object keys missing from `value` are deleted. */
  set(path: PathLike, value: unknown): void;
  /** Merges into the whole tree. */
  patch(value: Record<string, unknown>): void;
  /** Merges `value` into the value at `path`; nothing is deleted. */
  patch(path: PathLike, value: unknown): void;
  delete(path: PathLike): void;
}

/** What every hook gets. */
export interface BaseContext<C = unknown, E = Record<string, unknown>> {
  /** Connection ID, or `undefined` outside a host (in tests). */
  readonly conn: string | undefined;
  readonly alias: string;
  readonly mode: ConnMode;
  readonly config: C;
  readonly secrets: Readonly<Record<string, string | undefined>>;
  readonly state: StateApi;
  /** Emits an event. Events are never coalesced. */
  emit<K extends keyof E & string>(event: K, payload: E[K]): void;
  readonly log: Logger;
  /** Reports the connection status (spec §3.4). */
  status(status: ConnStatusKind, message?: string, detail?: unknown): void;
  /** Aborted when the connection is destroyed. */
  readonly signal: AbortSignal;
}

/** What live-mode actions get: the base context plus what `connect` returned. */
export interface Context<C = unknown, E = Record<string, unknown>, S = unknown>
  extends BaseContext<C, E> {
  readonly session: S;
}

/** The action call being run. */
export interface CallInfo {
  /** Call ID (ULID); the idempotency key. */
  readonly call: string;
  readonly run?: string;
  /** Aborted when the call's deadline passes or the connection is destroyed. */
  readonly signal: AbortSignal;
}

export interface ActionSpec<C, E, S, P> {
  name: string;
  description?: string;
  params: Schema<P>;
  result?: Schema<unknown>;
  idempotent?: boolean;
  cancellable?: boolean;
  durationHint?: "instant" | "short" | "long";
  learn?: boolean;
  /** Runs the action in live mode. The return value becomes the result. */
  run(ctx: Context<C, E, S>, params: P, call: CallInfo): unknown;
}

/** The state-level simulator (ADR 0012): used when `mode` is `simulate`. */
export interface SimulatorSpec<C, E, A> {
  /** Publishes believable initial state. */
  init?(sim: BaseContext<C, E>): void | Promise<void>;
  /** Answers actions with plausible state and events. */
  actions?: {
    [K in keyof A]?: (
      sim: BaseContext<C, E>,
      params: A[K],
      call: CallInfo,
    ) => unknown;
  };
}

export interface PluginDefinition<C, E, S, A> {
  /** Reverse-DNS ID, for example `io.covalence.videohub`. */
  id: string;
  name: string;
  version: string;
  description?: string;
  license?: string;
  authors?: string[];
  homepage?: string;
  categories?: string[];
  icon?: string;

  config: Schema<C>;
  /** Config schema version (ADR 0006). Defaults to 1. */
  configVersion?: number;
  configUi?: Record<string, unknown>;
  secrets?: Record<string, SecretDef>;
  /** Declared, not yet enforced (ADR 0015). */
  permissions?: Permissions;
  connections?: ConnectionPolicy;

  state: Record<string, Schema<unknown>>;
  events?: { [K in keyof E]: Schema<E[K]> };

  /**
   * Live mode: connects to the device. Whatever it returns is `ctx.session`
   * in actions and `disconnect`. When it resolves and the plugin hasn't
   * reported a status itself, the runtime sends a snapshot and `ok`.
   */
  connect?(ctx: BaseContext<C, E>): S | Promise<S>;
  /** Live mode: closes the device connection. */
  disconnect?(ctx: Context<C, E, S>, reason: string): void | Promise<void>;

  actions: { [K in keyof A]: ActionSpec<C, E, S, A[K]> };

  simulator?: SimulatorSpec<C, E, A>;
  /** The plugin ships a protocol-level simulator (a fake device server). */
  protocolSimulator?: boolean;
}

/** A defined plugin: its manifest plus the hooks the runtime calls. */
export interface Plugin {
  readonly manifest: Manifest;
  // deno-lint-ignore no-explicit-any
  readonly definition: PluginDefinition<any, any, any, any>;
}

/** Declares a plugin. The bundle's default export must be the result. */
export function definePlugin<
  C,
  E extends Record<string, unknown> = Record<string, never>,
  S = undefined,
  A extends Record<string, unknown> = Record<string, never>,
>(definition: PluginDefinition<C, E, S, A>): Plugin {
  return { manifest: buildManifest(definition), definition };
}

function buildManifest<C, E, S, A>(
  def: PluginDefinition<C, E, S, A>,
): Manifest {
  const [major, minor] = PROTOCOL_VERSION.split(".");
  const actions: ActionDef[] = Object.entries(
    def.actions as Record<string, ActionSpec<C, E, S, unknown>>,
  ).map(([id, a]) => ({
    id,
    name: a.name,
    description: a.description,
    params: a.params as JsonSchemaDoc,
    result: a.result as JsonSchemaDoc | undefined,
    idempotent: a.idempotent,
    cancellable: a.cancellable,
    durationHint: a.durationHint,
    learn: a.learn,
  }));
  const events: EventDef[] = Object.entries(
    (def.events ?? {}) as Record<string, Schema<unknown>>,
  ).map(([id, payload]) => ({
    id,
    name: (payload.title as string | undefined) ?? id,
    payload: payload as JsonSchemaDoc,
  }));
  return stripUndefined({
    id: def.id,
    name: def.name,
    version: def.version,
    protocol: `^${major}.${minor}`,
    sdk: SDK_VERSION,
    runtime: "deno",
    license: def.license ?? "MIT",
    authors: def.authors ?? [],
    homepage: def.homepage,
    categories: def.categories ?? [],
    icon: def.icon,
    config: {
      schema: def.config as JsonSchemaDoc,
      ui: def.configUi,
      version: def.configVersion ?? 1,
    },
    secrets: def.secrets,
    permissions: def.permissions ?? {},
    connections: def.connections ?? "multiple",
    actions,
    events,
    state: def.state,
    simulator: {
      stateLevel: def.simulator !== undefined,
      protocolLevel: def.protocolSimulator ?? false,
    },
  });
}

// Keeps the manifest structured-clone and JSON clean.
function stripUndefined<T>(value: T): T {
  return JSON.parse(JSON.stringify(value));
}

/** Throw from an action to fail it with a specific protocol error code. */
export class ActionError extends Error {
  constructor(
    /** Usually `action_failed`, `invalid_params`, `not_connected`, `timeout` or `cancelled` (spec §5.1). */
    readonly code: ErrorCode,
    message: string,
    readonly data?: unknown,
    readonly retryable?: boolean,
  ) {
    super(message);
    this.name = "ActionError";
  }
}
