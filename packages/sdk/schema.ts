/**
 * Schema builders. Each builder returns a plain JSON Schema object (what the
 * manifest carries) with a phantom type, so params and config are typed in
 * plugin code.
 *
 * State schemas use the same builders plus {@linkcode collection} (state spec
 * §1.2–1.4).
 *
 * @module
 */

import type { JsonSchemaDoc } from "@covalence-io/protocol";

/** A JSON Schema document that describes values of type `T`. */
export interface Schema<T = unknown> extends JsonSchemaDoc {
  /** Phantom field for type inference; never set. */
  readonly __type?: T;
}

/** The type a schema describes. */
export type Infer<S> = S extends Schema<infer T> ? T : never;

/** Annotations every builder accepts. */
export interface Common {
  /** Shown to people; emitted as `title`. */
  label?: string;
  description?: string;
  /** State spec §1.2. */
  kind?:
    | "duration"
    | "timestamp"
    | "color"
    | "enum"
    | "percent"
    | "id"
    | "target";
  unit?: string;
  readOnly?: boolean;
  /** Offer the keys of a state collection as choices: `{from: "outputs"}`. */
  choices?: { from: string };
}

interface NumberOpts extends Common {
  default?: number;
  minimum?: number;
  maximum?: number;
  step?: number;
}

interface StringOpts extends Common {
  default?: string;
  format?: string;
  enum?: readonly string[];
  minLength?: number;
}

const optionals = new WeakSet<object>();

function build(type: string, opts: Common = {}) {
  const { label, choices, ...rest } = opts as Common & Record<string, unknown>;
  const out: Record<string, unknown> = { type };
  if (label !== undefined) out.title = label;
  if (choices !== undefined) out["x-choices"] = choices;
  for (const [k, v] of Object.entries(rest)) if (v !== undefined) out[k] = v;
  return out;
}

export function string(opts?: StringOpts): Schema<string> {
  return build("string", opts);
}

export function number(opts?: NumberOpts): Schema<number> {
  return build("number", opts);
}

export function integer(opts?: NumberOpts): Schema<number> {
  return build("integer", opts);
}

export function boolean(
  opts?: Common & { default?: boolean },
): Schema<boolean> {
  return build("boolean", opts);
}

/** Marks an object property as optional. */
export function optional<T>(schema: Schema<T>): Schema<T | undefined> & {
  readonly __optional?: true;
} {
  const copy = { ...schema };
  optionals.add(copy);
  return copy;
}

type Props = Record<string, Schema<unknown>>;
type OptionalKeys<P extends Props> = {
  [K in keyof P]: P[K] extends { readonly __optional?: true } ? K : never;
}[keyof P];
type ObjectOf<P extends Props> =
  & { [K in Exclude<keyof P, OptionalKeys<P>>]: Infer<P[K]> }
  & { [K in OptionalKeys<P>]?: Infer<P[K]> };

/** An object. Properties are required unless wrapped in {@linkcode optional} or given a `default`. */
export function object<P extends Props>(
  props: P,
  opts?: Common,
): Schema<{ [K in keyof ObjectOf<P>]: ObjectOf<P>[K] }> {
  const required = Object.entries(props)
    .filter(([, s]) => !optionals.has(s) && !("default" in s))
    .map(([k]) => k);
  return {
    ...build("object", opts),
    properties: props,
    required,
    additionalProperties: false,
  };
}

/**
 * A dynamic collection in the state schema (state spec §1.4), for example a
 * router's outputs. Keys are integers or strings; `label` names the item field
 * shown to people.
 */
export function collection<P extends Props>(
  opts: { key: string; label?: string; description?: string },
  item: P,
): Schema<Record<string | number, Partial<ObjectOf<P>>>> {
  const { description, ...collectionDef } = opts;
  const out: Record<string, unknown> = { collection: collectionDef, item };
  if (description !== undefined) out.description = description;
  return out;
}

/**
 * Checks a value against the subset of JSON Schema these builders produce.
 * Returns a description of the first problem, or `undefined` if it is valid.
 */
export function validate(
  schema: JsonSchemaDoc,
  value: unknown,
  at = "",
): string | undefined {
  const where = at || "value";
  switch (schema.type) {
    case "object": {
      if (value === null || typeof value !== "object" || Array.isArray(value)) {
        return `${where} must be an object`;
      }
      const obj = value as Record<string, unknown>;
      const props = (schema.properties ?? {}) as Record<string, JsonSchemaDoc>;
      for (const key of (schema.required ?? []) as string[]) {
        if (obj[key] === undefined) {
          return `${at ? at + "." : ""}${key} is required`;
        }
      }
      for (const [key, v] of Object.entries(obj)) {
        const sub = props[key];
        if (!sub) {
          if (schema.additionalProperties === false) {
            return `${at ? at + "." : ""}${key} is not allowed`;
          }
          continue;
        }
        if (v === undefined) continue;
        const err = validate(sub, v, at ? `${at}.${key}` : key);
        if (err) return err;
      }
      return undefined;
    }
    case "string":
      if (typeof value !== "string") return `${where} must be a string`;
      if (
        Array.isArray(schema.enum) &&
        !(schema.enum as unknown[]).includes(value)
      ) {
        return `${where} must be one of ${
          (schema.enum as string[]).join(", ")
        }`;
      }
      if (
        typeof schema.minLength === "number" && value.length < schema.minLength
      ) {
        return `${where} is too short`;
      }
      return undefined;
    case "integer":
    case "number": {
      if (typeof value !== "number" || !Number.isFinite(value)) {
        return `${where} must be a number`;
      }
      if (schema.type === "integer" && !Number.isInteger(value)) {
        return `${where} must be an integer`;
      }
      if (typeof schema.minimum === "number" && value < schema.minimum) {
        return `${where} must be at least ${schema.minimum}`;
      }
      if (typeof schema.maximum === "number" && value > schema.maximum) {
        return `${where} must be at most ${schema.maximum}`;
      }
      return undefined;
    }
    case "boolean":
      return typeof value === "boolean"
        ? undefined
        : `${where} must be a boolean`;
    default:
      return undefined;
  }
}

/** Fills in `default`s of an object schema's properties. */
export function withDefaults(
  schema: JsonSchemaDoc,
  value: Record<string, unknown>,
): Record<string, unknown> {
  const out = { ...value };
  const props = (schema.properties ?? {}) as Record<string, JsonSchemaDoc>;
  for (const [key, sub] of Object.entries(props)) {
    if (out[key] === undefined && "default" in sub) out[key] = sub.default;
  }
  return out;
}
