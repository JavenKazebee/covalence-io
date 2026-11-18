// Variables and sinks (dataflow spec §6, §7). Mirrors
// `covalence_core::variable` and `covalence_core::dataflow::Sink` by hand.

import type { Problem } from "../flows/model";
import type { Json, Ref, Source } from "../sources/model";

/** A target: a connection ID, or `{conn, key}`. */
export type Target = string | { conn: string; key?: string };

export type Sink =
  | { set: Ref }
  | {
      action: {
        target: Source;
        action: string;
        /** The parameter that receives the value. */
        param: string;
        params?: Record<string, Source>;
        /** A duration: `20ms` when absent. */
        rateLimit?: string;
      };
    };

export interface Variable {
  /** Empty in a create request. */
  id: string;
  name: string;
  /** `vars.<slug>`. Derived from `name` by the server when empty. */
  slug: string;
  schema?: { type?: SchemaType; unit?: string; [k: string]: Json | undefined };
  initial?: Json;
  persist?: boolean;
  /** A computed or accumulated variable's source: read-only. */
  value?: Source;
  to?: Sink[];
  journal?: "full" | { sampled: string };
}

export interface SavedVariable {
  variable: Variable;
  warnings: Problem[];
}

/** A cell's problem (dataflow §5): `cycle`, an expression's code, `unsupported`. */
export interface CellProblem {
  /** `variable:<id>` or `helper:<binding>/<key>`. */
  cell: string;
  code: string;
  message: string;
}

export const schemaTypes = ["any", "boolean", "number", "integer", "string"] as const;
export type SchemaType = (typeof schemaTypes)[number];

/** The built-in reducers (dataflow §4.2), which keep a value of their own. */
export const reducers = ["toggle", "counter", "position", "velocity", "select", "absolute", "rate", "latch", "script"] as const;
export type ReducerName = (typeof reducers)[number];

export type Kind = "writable" | "computed" | "accumulated";

export const kindNames: Record<Kind, string> = {
  writable: "Writable: set by flows",
  computed: "Computed from a source",
  accumulated: "Accumulated by a reducer",
};

/** `toggle` from `toggle` or `toggle@1`, if a source is a reducer. */
export function reducerOf(source: Source | undefined): ReducerName | undefined {
  if (typeof source !== "object" || source === null || Array.isArray(source) || !("use" in source)) return undefined;
  const name = String(source.use).split("@")[0];
  return (reducers as readonly string[]).includes(name ?? "") ? (name as ReducerName) : undefined;
}

export function kindOf(v: Variable): Kind {
  if (v.value === undefined) return "writable";
  return reducerOf(v.value) !== undefined ? "accumulated" : "computed";
}

/** A variable changed to another kind: what the new kind doesn't use goes. */
export function withKind(v: Variable, kind: Kind): Variable {
  const rest = omit(v, ["value", "initial", "to"]);
  switch (kind) {
    case "writable":
      return { ...rest, ...(v.initial === undefined ? {} : { initial: v.initial }) };
    case "computed":
      return { ...rest, value: v.value !== undefined && kindOf(v) === "computed" ? v.value : { expr: "" }, ...(v.to ? { to: v.to } : {}) };
    case "accumulated":
      return { ...rest, value: v.value !== undefined && kindOf(v) === "accumulated" ? v.value : { use: "toggle", on: { from: "" } }, ...(v.to ? { to: v.to } : {}) };
  }
}

/** A reducer's options, everything but `use`, as the editor's JSON text shows them. */
export function reducerOptions(source: Source | undefined): Record<string, Json> {
  if (typeof source !== "object" || source === null || Array.isArray(source)) return {};
  return omit(source as Record<string, Json>, ["use"]);
}

/** `o` without `keys`. */
export function omit<T extends object, K extends keyof T>(o: T, keys: readonly K[]): Omit<T, K> {
  const out = { ...o };
  for (const k of keys) delete out[k];
  return out;
}

/** A reducer source from its name and options. */
export function reducerSource(name: ReducerName, options: Record<string, Json>): Source {
  return { ...options, use: name } as Source;
}

/** "sets vars.armed", "vh.setVolume(level), at most every 20ms". */
export function describeSink(sink: Sink, alias: (conn: string) => string): string {
  if ("set" in sink) {
    const ref = sink.set;
    const root = "root" in ref ? ref.root : alias(ref.conn);
    return `sets ${[root, ...ref.path].join(".")}`;
  }
  const a = sink.action;
  const t = a.target;
  const conn = typeof t === "string" ? alias(t) : typeof t === "object" && t !== null && "conn" in t ? alias(String(t.conn)) : "a target";
  return `${conn}.${a.action}(${a.param}), at most every ${a.rateLimit ?? "20ms"}`;
}

/** A new variable, writable and untyped. */
export function blankVariable(): Variable {
  return { id: "", name: "", slug: "" };
}

/** A path `vars.<name>` as a ref, or null if it isn't one. */
export function varsRef(text: string): Ref | null {
  const m = /^vars\.([A-Za-z_][A-Za-z0-9_]*)$/.exec(text.trim());
  return m === null ? null : { root: "vars", path: [m[1]!] };
}
