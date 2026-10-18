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
