// Devices and their traffic (plugin protocol §7, ADR 0025), as the UI shows them.

import type { DeviceEntry, JsonSchema, TrafficEntry } from "../api";

/**
 * The config property that names a connection's device: the first top-level
 * property with `format: "device"`, as the server picks it.
 */
export function deviceField(schema: JsonSchema): string | null {
  for (const [key, prop] of Object.entries(schema.properties ?? {})) {
    if (prop.format === "device") return key;
  }
  return null;
}

const ALIAS = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * An alias for a new connection with `plugin`: the last part of its ID,
 * numbered if that's taken, as the server makes them for `autoCreate`.
 */
export function freeAlias(plugin: string, taken: Iterable<string>): string {
  const used = new Set(taken);
  const last = plugin.slice(plugin.lastIndexOf(".") + 1);
  let base = last.replace(/[^A-Za-z0-9]/g, "_");
  if (!ALIAS.test(base)) base = `dev_${base}`;
  if (!used.has(base)) return base;
  for (let n = 2; ; n++) {
    if (!used.has(`${base}${n}`)) return `${base}${n}`;
  }
}

/** A device's name for people: its product, and its serial or path. */
export function deviceLabel(d: DeviceEntry): string {
  const name = d.product ?? d.manufacturer ?? d.kind.toUpperCase();
  return d.serial ? `${name} (${d.serial})` : `${name} at ${d.path}`;
}

/** `0x0fd9`. */
export function hex16(n: number | undefined): string {
  return n === undefined ? "" : `0x${n.toString(16).padStart(4, "0")}`;
}

function bytesOf(hex: string): number[] {
  const out: number[] = [];
  for (let i = 0; i + 1 < hex.length; i += 2) out.push(parseInt(hex.slice(i, i + 2), 16));
  return out;
}

/** `0a 1b ff`: the bytes, spaced. */
export function spacedHex(hex: string): string {
  return hex.replace(/(..)(?=.)/g, "$1 ");
}

/**
 * The bytes as text, for line protocols like the Videohub's: printable ASCII
 * as itself, `\n`, `\r` and `\t` escaped, anything else as `·`. Null when most
 * of it isn't text, so binary reports show as hex alone.
 */
export function asText(hex: string): string | null {
  const bytes = bytesOf(hex);
  if (bytes.length === 0) return null;
  const printable = bytes.filter((b) => (b >= 0x20 && b < 0x7f) || b === 0x0a || b === 0x0d || b === 0x09).length;
  if (printable / bytes.length < 0.9) return null;
  return bytes
    .map((b) => {
      if (b === 0x0a) return "\\n";
      if (b === 0x0d) return "\\r";
      if (b === 0x09) return "\\t";
      return b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "·";
    })
    .join("");
}

/** How many bytes the entry's message had, and whether `hex` holds fewer. */
export function sizeOf(entry: TrafficEntry): { len: number; cut: boolean } | null {
  if (entry.hex === undefined) return null;
  const shown = entry.hex.length / 2;
  const len = entry.len ?? shown;
  return { len, cut: len > shown };
}

/** `14:03:07.042`, in local time. */
export function clockTime(ts: number): string {
  const d = new Date(ts);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}
