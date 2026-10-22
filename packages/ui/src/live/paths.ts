/**
 * Whether text path `path` is `prefix` or below it. Segments start with `.`
 * or `[` (state spec §1.1), so `vh.out` is not under `vh.o`.
 */
export function isUnder(path: string, prefix: string): boolean {
  if (!path.startsWith(prefix)) return false;
  const next = path.charAt(prefix.length);
  return next === "" || next === "." || next === "[";
}

export function isUnderAny(path: string, prefixes: Iterable<string>): boolean {
  for (const prefix of prefixes) {
    if (isUnder(path, prefix)) return true;
  }
  return false;
}

const collator = new Intl.Collator("en", { numeric: true });

/** Orders text paths so `[2]` comes before `[10]`. */
export function comparePaths(a: string, b: string): number {
  return collator.compare(a, b);
}

export type Segment = string | number;

const IDENT = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Text form of a root and segments, as the server's `text_path::format` writes it. */
export function formatPath(root: string, segments: readonly Segment[]): string {
  let out = root;
  for (const segment of segments) {
    if (typeof segment === "number") out += `[${segment}]`;
    else if (IDENT.test(segment)) out += `.${segment}`;
    else out += `[${JSON.stringify(segment)}]`;
  }
  return out;
}

/**
 * Splits a text path into its root and segments. Paths come from the server,
 * so this trusts them: anything it can't read ends the path, and `end` is
 * where that happened (`text.length` when all of it was read).
 */
export function splitPath(text: string): { root: string; segments: Segment[]; end: number } {
  const ident = /^[A-Za-z_][A-Za-z0-9_]*/;
  const rootMatch = ident.exec(text);
  const root = rootMatch?.[0] ?? "";
  const segments: Segment[] = [];
  let at = root.length;
  while (at < text.length) {
    const ch = text[at];
    if (ch === ".") {
      const m = ident.exec(text.slice(at + 1));
      if (m === null) break;
      segments.push(m[0]);
      at += 1 + m[0].length;
    } else if (ch === "[") {
      const m = /^\[(\d+|"(?:[^"\\]|\\.)*")\]/.exec(text.slice(at));
      if (m === null || m[1] === undefined) break;
      segments.push(m[1].startsWith('"') ? (JSON.parse(m[1]) as string) : Number(m[1]));
      at += m[0].length;
    } else {
      break;
    }
  }
  return { root, segments, end: at };
}
