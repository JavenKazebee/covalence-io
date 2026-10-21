// Forms generated from the JSON Schemas in plugin manifests: connection config
// and action params. Flat objects of scalars are all M2 needs.

import type { JsonSchema } from "../api";
import { splitPath, type Segment } from "../live/paths";
import type { Leaf } from "../live/protocol";

export type FieldKind = "text" | "integer" | "number" | "boolean" | "enum" | "choices";

export interface Field {
  key: string;
  label: string;
  kind: FieldKind;
  required: boolean;
  schema: JsonSchema;
  /** For `choices`: the state collection to list, e.g. `inputs`. */
  from?: string;
}

/**
 * One field per property: required ones in `required` order, then the rest.
 * The server's JSON sorts property names, so `required` is the only order the
 * plugin author's intent survives in.
 */
export function fieldsOf(schema: JsonSchema): Field[] {
  const order = schema.required ?? [];
  const required = new Set(order);
  const rank = (key: string) => (required.has(key) ? order.indexOf(key) : order.length);
  const entries = Object.entries(schema.properties ?? {}).sort(([a], [b]) => rank(a) - rank(b));
  return entries.map(([key, prop]) => {
    const field: Field = {
      key,
      label: prop.title ?? key,
      kind: kindOf(prop),
      required: required.has(key),
      schema: prop,
    };
    if (field.kind === "choices") field.from = prop["x-choices"]?.from;
    return field;
  });
}

function kindOf(prop: JsonSchema): FieldKind {
  if (prop["x-choices"] !== undefined) return "choices";
  if (prop.enum !== undefined) return "enum";
  switch (prop.type) {
    case "integer":
      return "integer";
    case "number":
      return "number";
    case "boolean":
      return "boolean";
    default:
      return "text";
  }
}

/**
 * What the inputs start with: each property's default, as the text an input
 * holds. Booleans stay booleans.
 */
export function initialValues(fields: Field[]): Record<string, string | boolean> {
  const out: Record<string, string | boolean> = {};
  for (const field of fields) {
    const d = field.schema.default;
    if (field.kind === "boolean") out[field.key] = d === true;
    else out[field.key] = d === undefined ? "" : String(d);
  }
  return out;
}

export type Parsed =
  | { ok: true; value: Record<string, unknown> }
  | { ok: false; errors: Record<string, string> };

/**
 * Turns input text into the JSON the schema asks for. Empty optional fields
 * are left out so the server applies its defaults.
 */
export function parseValues(fields: Field[], raw: Record<string, string | boolean | undefined>): Parsed {
  const value: Record<string, unknown> = {};
  const errors: Record<string, string> = {};
  for (const field of fields) {
    const input = raw[field.key];
    if (field.kind === "boolean") {
      value[field.key] = input === true;
      continue;
    }
    const text = typeof input === "string" ? input.trim() : "";
    if (text === "") {
      if (field.required) errors[field.key] = "Required";
      continue;
    }
    const result = parseOne(field, text);
    if (typeof result === "string") errors[field.key] = result;
    else value[field.key] = result.value;
  }
  return Object.keys(errors).length > 0 ? { ok: false, errors } : { ok: true, value };
}

function parseOne(field: Field, text: string): { value: unknown } | string {
  const s = field.schema;
  if (field.kind === "enum") {
    const match = s.enum?.find((option) => String(option) === text);
    return match === undefined ? "Not one of the options" : { value: match };
  }
  if (field.kind === "text") {
    if (s.minLength !== undefined && text.length < s.minLength) return `At least ${s.minLength} characters`;
    return { value: text };
  }
  // integer, number, and choices (which are keys, so numbers when the schema says so).
  if (field.kind === "choices" && s.type !== "integer" && s.type !== "number") return { value: text };
  const n = Number(text);
  if (!Number.isFinite(n)) return "Not a number";
  if ((field.kind === "integer" || s.type === "integer") && !Number.isInteger(n)) return "Not a whole number";
  if (s.minimum !== undefined && n < s.minimum) return `At least ${s.minimum}`;
  if (s.maximum !== undefined && n > s.maximum) return `At most ${s.maximum}`;
  return { value: n };
}

export interface Choice {
  key: Segment;
  label: string;
}

/**
 * The items of a live state collection at `prefix`, e.g. `vh.inputs`, each
 * labelled by its `labelField` leaf when it has one.
 */
export function collectionChoices(prefix: string, leaves: readonly Leaf[], labelField?: string): Choice[] {
  const depth = splitPath(prefix).segments.length;
  const items = new Map<string, Choice>();
  for (const leaf of leaves) {
    const { segments } = splitPath(leaf.path);
    const key = segments[depth];
    if (key === undefined) continue;
    const id = JSON.stringify(key);
    const item = items.get(id) ?? { key, label: String(key) };
    items.set(id, item);
    if (labelField !== undefined && segments.length === depth + 2 && segments[depth + 1] === labelField) {
      item.label = `${key}: ${String(leaf.value)}`;
    }
  }
  return [...items.values()];
}
