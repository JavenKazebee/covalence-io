// Control bindings (bindings spec §1, §2): each property is a source.
// Mirrors `covalence_core::binding` by hand; the shared fixtures in
// crates/covalence-core/fixtures/bindings/ catch drift (overview spec: project entities).

import type { Problem } from "../flows/model";
import {
  copyJson,
  fallbackOf,
  formOf,
  formats,
  parseRef,
  refText,
  refsOf,
  type Aliases,
  type Expect,
  type Format,
  type Parsed,
  type Ref,
  type Source,
} from "../sources/model";

/** The properties the editor binds on a key, in the order the editor shows them. */
export const props = ["label", "textColor", "background"] as const;
export type Prop = (typeof props)[number];

export const propNames: Record<Prop, string> = {
  label: "Label",
  textColor: "Text colour",
  background: "Background",
};

/** What a property takes: text, or a `#rrggbb` / `#rgb` colour. */
export const propType = (prop: Prop): "text" | "color" => (prop === "label" ? "text" : "color");

/** The look of a key with nothing bound, as the renderer draws it (bindings spec §9.2). */
export const defaults: Record<Prop, string> = { label: "", textColor: "#ffffff", background: "#000000" };

export {
  copyJson,
  curves,
  formats,
  isRoot,
  literalText,
  ops,
  parseLiteral,
  parseRef,
  refText,
  roots,
  type Aliases,
  type Curve,
  type Format,
  type Json,
  type Op,
  type Parsed,
  type Ref,
  type Root,
} from "../sources/model";

/** A literal: what a fallback or a key's fixed look holds. */
export type Value = string | number | boolean | null;

/** A property's binding: any source (bindings spec §2, dataflow §1). */
export type Binding = Source;

export interface ControlBinding {
  /** Empty in a create request. */
  id: string;
  layer: string;
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

// ---- Templates as text ----

/** A template part the text form can show: text, a path, or a formatted path. */
function simplePart(part: Source): { text: string } | { ref: Ref; format?: Format } | null {
  if (typeof part === "string") return { text: part };
  const form = formOf(part);
  if (form === "ref" && fallbackOf(part) === undefined) return { ref: (part as { ref: Ref }).ref };
  if (form === "format" && fallbackOf(part) === undefined) {
    const t = part as { input: Source; format: Format };
    if (formOf(t.input) === "ref" && fallbackOf(t.input) === undefined) return { ref: (t.input as { ref: Ref }).ref, format: t.format };
  }
  return null;
}

/** Whether every part fits the text form. */
export const isSimpleTemplate = (parts: readonly Source[]) => parts.every((p) => simplePart(p) !== null);

const escapeText = (text: string) => text.replace(/[{}]/g, (c) => c + c);

/** `Out 1: {vh.outputs[1].source}`, with `{path|format}` for a format and `{{` for a brace. */
export function templateText(parts: readonly Source[], aliases: Aliases): string {
  return parts
    .map((part) => {
      const p = simplePart(part);
      if (p === null) return "{…}";
      if ("text" in p) return escapeText(p.text);
      return `{${refText(p.ref, aliases)}${p.format ? `|${p.format}` : ""}}`;
    })
    .join("");
}

/** Template parts from their text form. Adjacent text is one part, and empty text none. */
export function parseTemplate(text: string, aliases: Aliases): Parsed<Source[]> {
  const parts: Source[] = [];
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
    parts.push(bar === -1 ? { ref: ref.value } : { use: "format", input: { ref: ref.value }, format: format as Format });
    i = j + 1;
  }
  if (literal !== "") parts.push(literal);
  return { ok: true, value: parts };
}

// ---- Editing ----

/** What a property expects a source to give. */
export const expectOf = (prop: Prop): Expect => propType(prop);

/** A copy of a stored binding for the editor to change freely. */
export function toDraft(binding: ControlBinding): ControlBinding {
  return copyJson(binding);
}

/** The binding to send: properties in the editor's order. */
export function fromDraft(draft: ControlBinding): ControlBinding {
  const out: ControlBinding = { id: draft.id, layer: draft.layer, control: draft.control, props: {} };
  for (const prop of props) {
    const binding = draft.props[prop];
    if (binding !== undefined) out.props[prop] = copyJson(binding);
  }
  return out;
}

export function newDraft(layer: string, control: string): ControlBinding {
  return { id: "", layer, control, props: {} };
}

/** Every ref a binding reads directly, for showing which connections it needs. */
export const refsOfBinding = (binding: Binding): Ref[] => refsOf(binding);

/** Expands `#rgb` for `<input type="color">`, which takes only `#rrggbb`. */
export function fullHex(color: string): string | null {
  if (/^#[0-9a-f]{6}$/i.test(color)) return color.toLowerCase();
  const short = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/i.exec(color);
  return short === null ? null : `#${short.slice(1).map((c) => c + c).join("")}`.toLowerCase();
}
