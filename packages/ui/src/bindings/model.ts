// Control bindings: the part of the bindings spec M5 implements (§1, §2.1).
// Mirrors `covalence_core::binding` by hand; the shared fixtures in
// crates/covalence-core/fixtures/bindings/ catch drift (ADR 0020).

import { formatPath, splitPath, type Segment } from "../live/paths";
import type { Problem } from "../flows/model";

/** The properties M5 draws on a key, in the order the editor shows them. */
export const props = ["label", "textColor", "background"] as const;
export type Prop = (typeof props)[number];

export const propNames: Record<Prop, string> = {
  label: "Label",
  textColor: "Text colour",
  background: "Background",
};

/** What a property takes: text, or a `#rrggbb` / `#rgb` colour. */
export const propType = (prop: Prop): "text" | "color" => (prop === "label" ? "text" : "color");

/** The look of a key with nothing bound, as the renderer draws it (ADR 0023). */
export const defaults: Record<Prop, string> = { label: "", textColor: "#ffffff", background: "#000000" };

export const roots = ["vars", "sys", "surface", "page", "flow"] as const;
export type Root = (typeof roots)[number];

/** A state reference, stored by connection ID so renaming an alias breaks nothing. */
export type Ref = { conn: string; path: Segment[] } | { root: Root; path: Segment[] };

export const formats = ["auto", "mmss", "hhmmss", "percent", "fixed1", "fixed2", "upper"] as const;
export type Format = (typeof formats)[number];

export const ops = ["==", "!=", ">", ">=", "<", "<=", "truthy", "falsy"] as const;
export type Op = (typeof ops)[number];

export const curves = ["linear", "log", "exp"] as const;
export type Curve = (typeof curves)[number];

/** A literal: what a fallback, a rule's result or a condition holds. */
export type Value = string | number | boolean | null;

export interface Condition {
  path: Ref;
  op: Op;
  value?: Value;
}

export interface Rule {
  when: Condition;
  value: Value;
}

export type TemplatePart = string | { path: Ref; format?: Format };

export type Binding =
  | { kind: "value"; path: Ref; format?: Format; fallback?: Value }
  | { kind: "template"; parts: TemplatePart[]; fallback?: string }
  | { kind: "rules"; rules: Rule[]; else: Value }
  | { kind: "map"; path: Ref; from: [number, number]; to: [number, number] | Value[]; curve?: Curve; clamp?: boolean }
  | { kind: "expr"; source: string; fallback?: Value };

export type BindingKind = Binding["kind"];
export const kinds: readonly BindingKind[] = ["value", "template", "rules", "map", "expr"];

export const kindNames: Record<BindingKind, string> = {
  value: "Value",
  template: "Template",
  rules: "Rules",
  map: "Map",
  expr: "Expression",
};

export interface ControlBinding {
  /** Empty in a create request. */
  id: string;
  page: string;
  control: string;
  props: Partial<Record<Prop, Binding>>;
}

export interface SavedBinding {
  binding: ControlBinding;
  warnings: Problem[];
}

/** `POST /control-bindings/preview` (bindings spec §2.2). */
export interface Preview {
  look: { label: string; textColor: string; background: string; stale: boolean };
  props: Partial<Record<Prop, { value?: string; error?: string; expr?: string; reads: string[] }>>;
  /** Base64 PNG. */
  image: string;
}

/** `GET /control-bindings/problems`: a property that failed when last evaluated. */
export interface RuntimeProblem {
  binding: string;
  prop: Prop;
  message: string;
}

/** A deep copy of plain JSON. Unlike `structuredClone`, it takes Vue's reactive proxies. */
export const copyJson = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

// ---- Refs as text ----

/** How the editor names connections: alias from ID, and back. */
export interface Aliases {
  alias(conn: string): string | undefined;
  conn(alias: string): string | undefined;
}

export function isRoot(text: string): text is Root {
  return (roots as readonly string[]).includes(text);
}

/** `vh.outputs[1].source`, with the connection by alias (its ID if it's gone). */
export function refText(ref: Ref, aliases: Aliases): string {
  if ("root" in ref) return formatPath(ref.root, ref.path);
  return formatPath(aliases.alias(ref.conn) ?? ref.conn, ref.path);
}

export type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

/** A ref from its text form: a reserved root, or a connection alias. */
export function parseRef(text: string, aliases: Aliases): Parsed<Ref> {
  const trimmed = text.trim();
  if (trimmed === "") return { ok: false, error: "Required" };
  const { root, segments, end } = splitPath(trimmed);
  if (root === "" || end !== trimmed.length) return { ok: false, error: `Can't read the path after "${trimmed.slice(0, end)}"` };
  if (isRoot(root)) return { ok: true, value: { root, path: segments } };
  const conn = aliases.conn(root);
  if (conn === undefined) return { ok: false, error: `No connection ${root}` };
  return { ok: true, value: { conn, path: segments } };
}

// ---- Templates as text ----

const escapeText = (text: string) => text.replace(/[{}]/g, (c) => c + c);

/** `Out 1: {vh.outputs[1].source}`, with `{path|format}` for a format and `{{` for a brace. */
export function templateText(parts: readonly TemplatePart[], aliases: Aliases): string {
  return parts
    .map((part) =>
      typeof part === "string" ? escapeText(part) : `{${refText(part.path, aliases)}${part.format ? `|${part.format}` : ""}}`,
    )
    .join("");
}

/** Template parts from their text form. Adjacent text is one part, and empty text none. */
export function parseTemplate(text: string, aliases: Aliases): Parsed<TemplatePart[]> {
  const parts: TemplatePart[] = [];
  let literal = "";
  let i = 0;
  while (i < text.length) {
    const ch = text[i]!;
    if ((ch === "{" || ch === "}") && text[i + 1] === ch) {
      literal += ch;
      i += 2;
      continue;
    }
    if (ch === "}") return { ok: false, error: "A } needs a { before it, or write }} for the brace itself" };
    if (ch !== "{") {
      literal += ch;
      i += 1;
      continue;
    }
    // A placeholder runs to the first `}` outside a quoted key.
    let j = i + 1;
    let quoted = false;
    for (; j < text.length; j++) {
      const c = text[j];
      if (quoted && c === "\\") j++;
      else if (c === '"') quoted = !quoted;
      else if (c === "}" && !quoted) break;
    }
    if (j >= text.length) return { ok: false, error: "A { needs a } after it, or write {{ for the brace itself" };
    const inner = text.slice(i + 1, j);
    const bar = inner.lastIndexOf("|");
    const format = bar === -1 ? "" : inner.slice(bar + 1).trim();
    if (bar !== -1 && !(formats as readonly string[]).includes(format)) {
      return { ok: false, error: `No format ${format}: use ${formats.join(", ")}` };
    }
    const ref = parseRef(bar === -1 ? inner : inner.slice(0, bar), aliases);
    if (!ref.ok) return { ok: false, error: `{${inner}}: ${ref.error}` };
    if (literal !== "") parts.push(literal);
    literal = "";
    parts.push(bar === -1 ? { path: ref.value } : { path: ref.value, format: format as Format });
    i = j + 1;
  }
  if (literal !== "") parts.push(literal);
  return { ok: true, value: parts };
}

// ---- Literals as text ----

/**
 * A condition's value as text. Strings show as typed unless they'd read back
 * as something else (`3`, `true`), when they're quoted.
 */
export function literalText(value: Value | undefined): string {
  if (value === undefined) return "";
  if (typeof value !== "string") return JSON.stringify(value);
  return parseLiteral(value) === value ? value : JSON.stringify(value);
}

/** A literal from text: JSON when it reads as JSON, else the text itself. Empty is no value. */
export function parseLiteral(text: string): Value | undefined {
  const trimmed = text.trim();
  if (trimmed === "") return undefined;
  try {
    const value: unknown = JSON.parse(trimmed);
    if (value === null || ["string", "number", "boolean"].includes(typeof value)) return value as Value;
  } catch {
    // Not JSON: plain text.
  }
  return text;
}

// ---- Editing ----

/** A fresh binding of `kind` for `prop`, ready to fill in. */
export function newBinding(kind: BindingKind, prop: Prop): Binding {
  const blank = defaults[prop];
  const path: Ref = { root: "vars", path: [] };
  switch (kind) {
    case "value":
      return { kind, path };
    case "template":
      return { kind, parts: [] };
    case "rules":
      return { kind, rules: [], else: prop === "label" ? "" : blank };
    case "map":
      return { kind, path, from: [0, 1], to: prop === "label" ? [0, 100] : [blank, "#ffffff"] };
    case "expr":
      return { kind, source: "" };
  }
}

/** The fallback a kind can carry, if it can. */
export function fallbackOf(binding: Binding): Value | undefined {
  return binding.kind === "rules" || binding.kind === "map" ? undefined : binding.fallback;
}

/**
 * A binding converted to `expr` (bindings spec §2.2): the compiled source,
 * keeping the fallback where the kind had one. Converting is one-way unless
 * the source is left as it was.
 */
export function toExpr(binding: Binding, compiled: string): Binding {
  const fallback = fallbackOf(binding);
  return fallback === undefined ? { kind: "expr", source: compiled } : { kind: "expr", source: compiled, fallback };
}

/** A copy of a stored binding for the editor to change freely. */
export function toDraft(binding: ControlBinding): ControlBinding {
  return copyJson(binding);
}

/** The binding to send: properties in the editor's order, and none left empty. */
export function fromDraft(draft: ControlBinding): ControlBinding {
  const out: ControlBinding = { id: draft.id, page: draft.page, control: draft.control, props: {} };
  for (const prop of props) {
    const binding = draft.props[prop];
    if (binding !== undefined) out.props[prop] = copyJson(binding);
  }
  return out;
}

export function newDraft(page: string, control: string): ControlBinding {
  return { id: "", page, control, props: {} };
}

/** Every ref a binding reads, for showing which connections it needs. */
export function refsOf(binding: Binding): Ref[] {
  switch (binding.kind) {
    case "value":
    case "map":
      return [binding.path];
    case "template":
      return binding.parts.flatMap((p) => (typeof p === "string" ? [] : [p.path]));
    case "rules":
      return binding.rules.map((r) => r.when.path);
    case "expr":
      return [];
  }
}

/** Expands `#rgb` for `<input type="color">`, which takes only `#rrggbb`. */
export function fullHex(color: string): string | null {
  if (/^#[0-9a-f]{6}$/i.test(color)) return color.toLowerCase();
  const short = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/i.exec(color);
  return short === null ? null : `#${short.slice(1).map((c) => c + c).join("")}`.toLowerCase();
}
