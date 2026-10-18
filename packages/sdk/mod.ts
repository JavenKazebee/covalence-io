/**
 * The Covalence IO plugin SDK.
 *
 * ```ts
 * import { definePlugin, s } from "@covalence-io/sdk";
 * ```
 *
 * The connection runtime (used by the plugin host and by plugin tests) is at
 * `@covalence-io/sdk/runtime`; the protocol-level simulator toolkit is at
 * `@covalence-io/sdk/simulator`.
 *
 * @module
 */

export { PROTOCOL_VERSION } from "@covalence-io/protocol";
export * as s from "./schema.ts";
export type { Infer, Schema } from "./schema.ts";
export {
  ActionError,
  type ActionSpec,
  type BaseContext,
  type CallInfo,
  type Context,
  definePlugin,
  type Logger,
  type Plugin,
  type PluginDefinition,
  SDK_VERSION,
  type SimulatorSpec,
  type StateApi,
} from "./plugin.ts";
export {
  formatPath,
  parsePath,
  type Path,
  type PathLike,
  toPath,
} from "./paths.ts";
