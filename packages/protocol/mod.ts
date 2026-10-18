/**
 * Plugin protocol types and framing.
 *
 * Types are generated from the schema in `protocol/` (ADR 0016).
 *
 * @module
 */

export type * from "./types.gen.ts";
export type * from "./methods.ts";
export * from "./framing.ts";

/** Protocol version this build speaks. */
export const PROTOCOL_VERSION = "0.1.0";
