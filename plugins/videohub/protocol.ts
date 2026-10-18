/**
 * The Blackmagic Videohub Ethernet protocol (TCP port 9990): text blocks of
 * lines, each block ended by a blank line.
 *
 * ```
 * VIDEO OUTPUT ROUTING:
 * 3 5
 *
 * ```
 *
 * A block starts with a header ending in `:`, except the `ACK` and `NAK`
 * replies. Port, input and output numbers are zero-based.
 *
 * @module
 */

export interface Block {
  /** The header without its colon, or `ACK` / `NAK`. */
  header: string;
  lines: string[];
}

export const PRELUDE_END = "END PRELUDE";

/** Splits a text stream into blocks. */
export class BlockDecoder {
  #text = "";
  #lines: string[] = [];
  readonly #decoder = new TextDecoder();

  /** Appends bytes and returns every block they complete. */
  push(data: Uint8Array | string): Block[] {
    this.#text += typeof data === "string"
      ? data
      : this.#decoder.decode(data, { stream: true });
    const out: Block[] = [];
    let nl: number;
    while ((nl = this.#text.indexOf("\n")) >= 0) {
      const line = this.#text.slice(0, nl).replace(/\r$/, "");
      this.#text = this.#text.slice(nl + 1);
      if (line === "") {
        if (this.#lines.length > 0) out.push(toBlock(this.#lines));
        this.#lines = [];
      } else {
        this.#lines.push(line);
      }
    }
    return out;
  }
}

function toBlock(lines: string[]): Block {
  const [first, ...rest] = lines;
  return {
    header: first.endsWith(":") ? first.slice(0, -1) : first,
    lines: rest,
  };
}

/** Formats a block, including its terminating blank line. */
export function formatBlock(header: string, lines: string[] = []): string {
  const head = header === "ACK" || header === "NAK" ? header : `${header}:`;
  return [head, ...lines, "", ""].join("\n");
}

/** Parses `Key: value` lines (`VIDEOHUB DEVICE`, `PROTOCOL PREAMBLE`). */
export function parseFields(lines: string[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const line of lines) {
    const colon = line.indexOf(":");
    if (colon > 0) out.set(line.slice(0, colon), line.slice(colon + 1).trim());
  }
  return out;
}

/** Parses `<index> <label>` lines (`INPUT LABELS`, `OUTPUT LABELS`). */
export function parseLabels(lines: string[]): Map<number, string> {
  const out = new Map<number, string>();
  for (const line of lines) {
    const m = /^(\d+) (.*)$/.exec(line);
    if (m) out.set(Number(m[1]), m[2]);
  }
  return out;
}

/** Parses `<output> <input>` lines (`VIDEO OUTPUT ROUTING`). */
export function parseRoutes(lines: string[]): Map<number, number> {
  const out = new Map<number, number>();
  for (const line of lines) {
    const m = /^(\d+) (\d+)$/.exec(line.trim());
    if (m) out.set(Number(m[1]), Number(m[2]));
  }
  return out;
}
