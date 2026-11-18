// Sources and stateless transforms (dataflow spec §1, §4.1, dataflow spec §10.1).
// Mirrors `covalence_core::source` and `covalence_core::transform` by hand;
// the shared fixtures catch drift (overview spec: project entities).

import { formatPath, splitPath, type Segment } from "../live/paths";

/** Plain JSON, as a literal holds it. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export const roots = ["vars", "sys", "surface", "layer", "flow"] as const;
export type Root = (typeof roots)[number];

/** A state reference, stored by connection ID so renaming an alias breaks nothing. */
export type Ref = { conn: string; path: Segment[] } | { root: Root; path: Segment[] };

export const formats = ["auto", "mmss", "hhmmss", "percent", "fixed1", "fixed2", "upper"] as const;
export type Format = (typeof formats)[number];

export const ops = ["==", "!=", ">", ">=", "<", "<=", "truthy", "falsy"] as const;
export type Op = (typeof ops)[number];

export const curves = ["linear", "log", "exp"] as const;
export type Curve = (typeof curves)[number];

export interface Rule {
  when: Source;
  value: Source;
}

export type Transform =
  | { use: "format"; input: Source; format: Format }
  | { use: "template"; parts: Source[] }
  | { use: "rules"; rules: Rule[]; else: Source }
  | { use: "compare"; input: Source; op: Op; value?: Source }
  | { use: "all"; of: Source[] }
  | { use: "any"; of: Source[] }
  | { use: "not"; input: Source }
  | { use: "map"; input: Source; from: [number, number]; to: [number, number] | Json[]; curve?: Curve; clamp?: boolean };

export type Use = Transform["use"];

/** Where a value comes from: a literal, a path, an expression or a transform. */
export type Source =
  | Json
  | { value: Json }
  | { ref: Ref; fallback?: Json }
  | { expr: string; fallback?: Json }
  | (Transform & { fallback?: Json });

/** The forms a source can take, as the editor offers them. */
export type Form = "literal" | "ref" | "expr" | Use;

export const uses: readonly Use[] = ["format", "template", "rules", "map", "compare", "all", "any", "not"];

export const formNames: Record<Form, string> = {
  literal: "Value",
  ref: "Path",
  expr: "Expression",
  format: "Formatted",
  template: "Template",
  rules: "Rules",
  map: "Map",
  compare: "Compare",
  all: "All of",
  any: "Any of",
  not: "Not",
};

const reserved = ["ref", "expr", "use", "value"] as const;

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** Whether a value would read as a source, so a literal of it needs `{value}`. */
export const looksLikeSource = (v: unknown): boolean => isObject(v) && reserved.some((k) => k in v);

/** A source's form, by its shape (dataflow §1.1): `use` decides by itself. */
export function formOf(source: Source): Form {
  if (!isObject(source)) return "literal";
  if ("use" in source) return source.use as Use;
  if ("ref" in source) return "ref";
  if ("expr" in source) return "expr";
  return "literal";
}

/** A literal source's value, `{value}` unwrapped. */
export function literalOf(source: Source): Json {
  if (isObject(source) && "value" in source && !("use" in source)) return (source as { value: Json }).value;
  return source as Json;
}

/** A literal, escaped with `{value}` when it would read as a source. */
export const literal = (v: Json): Source => (looksLikeSource(v) ? { value: v } : v);

export function fallbackOf(source: Source): Json | undefined {
  return isObject(source) && formOf(source) !== "literal" ? (source as { fallback?: Json }).fallback : undefined;
}

/** A copy of `source` with `fallback` set, or removed for `undefined`. */
export function withFallback(source: Source, fallback: Json | undefined): Source {
  if (formOf(source) === "literal") return source;
  const out = { ...(source as Record<string, unknown>) };
  if (fallback === undefined) delete out.fallback;
  else out.fallback = fallback;
  return out as Source;
}

/** What a source is expected to give, so a new one starts sensibly. */
export type Expect = "text" | "color" | "boolean" | "number" | "any";

/** A fresh source of `form`, ready to fill in. */
export function newSource(form: Form, expect: Expect = "any"): Source {
  const path: Ref = { root: "vars", path: [] };
  const blank: Json = expect === "color" ? "#000000" : expect === "number" ? 0 : expect === "boolean" ? true : "";
  switch (form) {
    case "literal":
      return blank;
    case "ref":
      return { ref: path };
    case "expr":
      return { expr: "" };
    case "format":
      return { use: "format", input: { ref: path }, format: "auto" };
    case "template":
      return { use: "template", parts: [] };
    case "rules":
      return { use: "rules", rules: [], else: blank };
    case "map":
      return expect === "color" || expect === "text"
        ? { use: "map", input: { ref: path }, from: [0, 1], to: expect === "color" ? ["#000000", "#ffffff"] : [0, 100] }
        : { use: "map", input: { ref: path }, from: [0, 1], to: [0, 100] };
    case "compare":
      return { use: "compare", input: { ref: path }, op: "==", value: 1 };
    case "all":
    case "any":
      return { use: form, of: [] };
    case "not":
      return { use: "not", input: { ref: path } };
  }
}

/** The sources nested in a transform. */
export function childrenOf(source: Source): Source[] {
  if (!isObject(source) || !("use" in source)) return [];
  const t = source as Transform;
  switch (t.use) {
    case "format":
    case "not":
    case "map":
      return [t.input];
    case "compare":
      return t.value === undefined ? [t.input] : [t.input, t.value];
    case "template":
      return t.parts;
    case "all":
    case "any":
      return t.of;
    case "rules":
      return [...t.rules.flatMap((r) => [r.when, r.value]), t.else];
  }
}

/** Every path a source reads directly, nested transforms included. */
export function refsOf(source: Source): Ref[] {
  if (formOf(source) === "ref") return [(source as { ref: Ref }).ref];
  return childrenOf(source).flatMap(refsOf);
}

/** A deep copy of plain JSON. Unlike `structuredClone`, it takes Vue's reactive proxies. */
export const copyJson = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

// ---- Paths as text ----

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

// ---- Compiled expressions (dataflow §4.1) ----
// The same text `covalence_core::transform::to_expr` writes, so a nested
// source can be edited as an expression before the core has seen it.

/** JavaScript's `Number.prototype.toString` for the numbers a map holds. */
const num = (x: number) => String(x);

const literalExpr = (v: Json) => (isObject(v) ? `(${JSON.stringify(v)})` : JSON.stringify(v));

const escapeTemplate = (text: string) => text.replace(/\\/g, "\\\\").replace(/`/g, "\\`").replace(/\$\{/g, "\\${");

function formatExpr(v: string, format: Format): string {
  switch (format) {
    case "auto":
      return `String(${v})`;
    case "mmss":
      return `mmss(${v})`;
    case "hhmmss":
      return `hhmmss(${v})`;
    case "percent":
      return `\`\${Math.round(Number(${v}) * 100)}%\``;
    case "fixed1":
      return `Number(${v}).toFixed(1)`;
    case "fixed2":
      return `Number(${v}).toFixed(2)`;
    case "upper":
      return `String(${v}).toUpperCase()`;
  }
}

/**
 * The expression a source stands for, without its own fallback, with
 * connections by alias. `null` if a connection has no alias.
 */
export function toExpr(source: Source, aliases: Aliases): string | null {
  switch (formOf(source)) {
    case "literal":
      return literalExpr(literalOf(source));
    case "ref": {
      const ref = (source as { ref: Ref }).ref;
      if ("conn" in ref && aliases.alias(ref.conn) === undefined) return null;
      return refText(ref, aliases);
    }
    case "expr":
      return (source as { expr: string }).expr;
    default:
      return transformExpr(source as Transform, aliases);
  }
}

/** A nested source as an operand: its fallback applied, in parentheses unless it's a literal or a path. */
function operand(source: Source, aliases: Aliases): string | null {
  const text = toExpr(source, aliases);
  if (text === null) return null;
  const form = formOf(source);
  const wrapped = form === "literal" || form === "ref" ? text : `(${text})`;
  const fallback = fallbackOf(source);
  if (fallback === undefined) return wrapped;
  const f = literalExpr(fallback);
  return `(() => { try { const v = ${wrapped}; return v === undefined ? ${f} : v; } catch { return ${f}; } })()`;
}

function transformExpr(t: Transform, aliases: Aliases): string | null {
  const op = (s: Source) => operand(s, aliases);
  const all = (items: Source[]) => {
    const out = items.map(op);
    return out.includes(null) ? null : (out as string[]);
  };
  switch (t.use) {
    case "format": {
      const input = op(t.input);
      return input === null ? null : `((v) => v === undefined ? undefined : ${formatExpr("v", t.format)})(${input})`;
    }
    case "template": {
      const values: string[] = [];
      let text = "`";
      for (const part of t.parts) {
        if (typeof part === "string") text += escapeTemplate(part);
        else {
          const v = op(part);
          if (v === null) return null;
          text += `\${String(v${values.length})}`;
          values.push(v);
        }
      }
      text += "`";
      if (values.length === 0) return text;
      const names = values.map((_, i) => `v${i}`);
      return `((${names.join(", ")}) => ${names.map((n) => `${n} === undefined`).join(" || ")} ? undefined : ${text})(${values.join(", ")})`;
    }
    case "rules": {
      let out = "";
      for (const rule of t.rules) {
        const when = op(rule.when);
        const value = op(rule.value);
        if (when === null || value === null) return null;
        out += `${when} ? ${value} : `;
      }
      const otherwise = op(t.else);
      return otherwise === null ? null : out + otherwise;
    }
    case "compare": {
      const input = op(t.input);
      const value = t.value === undefined ? "undefined" : op(t.value);
      if (input === null || value === null) return null;
      const sign: Record<Op, string> = {
        "==": "===",
        "!=": "!==",
        ">": ">",
        ">=": ">=",
        "<": "<",
        "<=": "<=",
        truthy: "",
        falsy: "",
      };
      if (t.op === "truthy") return `!!${input}`;
      if (t.op === "falsy") return `!${input}`;
      return `${input} ${sign[t.op]} ${value}`;
    }
    case "all":
    case "any": {
      if (t.of.length === 0) return t.use === "all" ? "true" : "false";
      const items = all(t.of);
      return items === null ? null : items.map((i) => `!!${i}`).join(t.use === "all" ? " && " : " || ");
    }
    case "not": {
      const input = op(t.input);
      return input === null ? null : `!${input}`;
    }
    case "map": {
      const input = op(t.input);
      if (input === null) return null;
      const [a, b] = t.from;
      let body = `let t = (v - ${num(a)}) / (${num(b)} - ${num(a)});`;
      if (t.clamp !== false) body += " t = Math.min(Math.max(t, 0), 1);";
      if (t.curve === "log") body += " t = Math.log10(1 + 9 * t);";
      if (t.curve === "exp") body += " t = (Math.pow(10, t) - 1) / 9;";
      const range = t.to.length === 2 && t.to.every((v) => typeof v === "number");
      if (range) {
        const [c, d] = t.to as [number, number];
        body += ` return ${num(c)} + (${num(d)} - ${num(c)}) * t;`;
      } else {
        body += ` const to = ${JSON.stringify(t.to)}; return to[Math.min(Math.max(Math.floor(t * to.length), 0), to.length - 1)];`;
      }
      return `((v) => { if (typeof v !== "number") return undefined; ${body} })(${input})`;
    }
  }
}

// ---- Literals as text ----

/**
 * A literal as text. Strings show as typed unless they'd read back as
 * something else (`3`, `true`), when they're quoted.
 */
export function literalText(value: Json | undefined): string {
  if (value === undefined) return "";
  if (typeof value !== "string") return JSON.stringify(value);
  return parseLiteral(value) === value ? value : JSON.stringify(value);
}

/** A literal from text: JSON when it reads as JSON, else the text itself. Empty is no value. */
export function parseLiteral(text: string): Json | undefined {
  const trimmed = text.trim();
  if (trimmed === "") return undefined;
  try {
    return JSON.parse(trimmed) as Json;
  } catch {
    return text;
  }
}
