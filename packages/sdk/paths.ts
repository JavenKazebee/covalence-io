/**
 * State paths. On the wire a path is an array of segments (ADR 0017); plugin
 * code may also write the text form people use:
 *
 * - `outputs[3].source` → `["outputs", 3, "source"]`
 * - `outputs["SDI 4"].source` → `["outputs", "SDI 4", "source"]`
 *
 * @module
 */

import type { PathSegment } from "@covalence-io/protocol";

export type Path = PathSegment[];

/** A path in either form. */
export type PathLike = string | readonly PathSegment[];

const IDENT = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Converts a path to segments, parsing the text form if needed. */
export function toPath(path: PathLike): Path {
  if (typeof path !== "string") {
    for (const seg of path) checkSegment(seg);
    return [...path];
  }
  return parsePath(path);
}

function checkSegment(seg: unknown) {
  if (typeof seg === "string") return;
  if (typeof seg === "number" && Number.isSafeInteger(seg)) return;
  throw new TypeError(`invalid path segment: ${String(seg)}`);
}

/** Parses the text form. The empty string is the root path. */
export function parsePath(text: string): Path {
  const out: Path = [];
  let i = 0;
  const fail = (why: string): never => {
    throw new SyntaxError(
      `invalid path ${JSON.stringify(text)}: ${why} at ${i}`,
    );
  };
  while (i < text.length) {
    if (text[i] === "[") {
      i++;
      const quote = text[i];
      if (quote === '"' || quote === "'") {
        i++;
        let key = "";
        for (;;) {
          if (i >= text.length) fail("unterminated string");
          const c = text[i++];
          if (c === quote) break;
          if (c === "\\") {
            if (i >= text.length) fail("unterminated escape");
            key += text[i++];
          } else {
            key += c;
          }
        }
        out.push(key);
      } else {
        const start = i;
        if (text[i] === "-") i++;
        while (i < text.length && text[i] >= "0" && text[i] <= "9") i++;
        const digits = text.slice(start, i);
        if (!/^-?\d+$/.test(digits)) fail("expected an integer or a string");
        const n = Number(digits);
        if (!Number.isSafeInteger(n)) fail("integer out of range");
        out.push(n);
      }
      if (text[i] !== "]") fail("expected ]");
      i++;
    } else {
      if (out.length > 0) {
        if (text[i] !== ".") fail("expected . or [");
        i++;
      }
      const start = i;
      while (i < text.length && text[i] !== "." && text[i] !== "[") i++;
      const name = text.slice(start, i);
      if (!IDENT.test(name)) fail("expected an identifier");
      out.push(name);
    }
  }
  return out;
}

/** Formats segments in the text form. */
export function formatPath(path: readonly PathSegment[]): string {
  let out = "";
  for (const seg of path) {
    if (typeof seg === "number") out += `[${seg}]`;
    else if (IDENT.test(seg)) out += out ? `.${seg}` : seg;
    else out += `[${JSON.stringify(seg)}]`;
  }
  return out;
}

/** A string key that tells `3` and `"3"` apart, for maps keyed by path. */
export function pathKey(path: readonly PathSegment[]): string {
  return JSON.stringify(path);
}

/** True if `prefix` is `path` or one of its ancestors. */
export function isPrefix(
  prefix: readonly PathSegment[],
  path: readonly PathSegment[],
): boolean {
  if (prefix.length > path.length) return false;
  return prefix.every((seg, i) => seg === path[i]);
}
