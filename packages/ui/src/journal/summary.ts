// One-line descriptions of journal entries (state spec §2.2).

import { formatPath, type Segment } from "../live/paths";
import type { JournalEntry, Origin } from "../live/protocol";

export interface SummaryContext {
  /** A connection's alias from its ID. */
  alias(connId: string): string;
  /** `vh.route` for an action call ID, when its `action.requested` entry is known. */
  callName?(call: string): string | undefined;
}

type Data = Record<string, unknown>;

interface IdPath {
  conn: string;
  path: Segment[];
}

const json = (value: unknown, max = 80) => clip(JSON.stringify(value) ?? "", max);

function clip(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max - 1)}…`;
}

function textPath(p: IdPath | undefined, ctx: SummaryContext): string {
  return p === undefined ? "?" : formatPath(ctx.alias(p.conn), p.path ?? []);
}

export function describeOrigin(origin: Origin, ctx: SummaryContext): string {
  if ("user" in origin) return `${origin.user} (${origin.client})`;
  if ("conn" in origin) return ctx.alias(origin.conn);
  if ("surface" in origin) return `surface ${origin.surface}`;
  if ("flow" in origin) return `flow ${origin.flow}`;
  return `core ${origin.core}`;
}

export function describe(entry: JournalEntry, ctx: SummaryContext): string {
  const d = (entry.data ?? {}) as Data;
  const call = () => {
    const id = String(d.call ?? "");
    return ctx.callName?.(id) ?? `call …${id.slice(-6)}`;
  };
  switch (entry.kind) {
    case "log":
      return `[${String(d.level ?? "info")}] ${String(d.message ?? "")}`;
    case "conn.status":
      return `${ctx.alias(String(d.conn))} ${String(d.status)}${d.message ? `: ${String(d.message)}` : ""}`;
    case "state.patch": {
      const ops = (d.ops ?? []) as ({ path: IdPath; value?: unknown; delete?: boolean })[];
      const shown = ops
        .slice(0, 2)
        .map((op) => (op.delete ? `${textPath(op.path, ctx)} deleted` : `${textPath(op.path, ctx)} = ${json(op.value, 40)}`));
      const more = ops.length > 2 ? ` and ${ops.length - 2} more` : "";
      return shown.join(", ") + more;
    }
    case "state.quality":
      return `${textPath(d.subtree as IdPath, ctx)} ${String(d.quality)}`;
    case "event":
      return `${String(d.event)} ${json(d.payload)}`;
    case "action.requested": {
      const name = `${ctx.alias(String(d.conn))}.${String(d.action)}`;
      return `${name} ${json(d.params)}`;
    }
    case "action.dispatched":
      return `${call()} dispatched`;
    case "action.suppressed":
      return `${call()} suppressed: ${String(d.reason)}`;
    case "action.result": {
      const took = typeof d.durationMs === "number" ? ` in ${d.durationMs} ms` : "";
      if (d.ok === true) return `${call()} OK${took}${d.result == null ? "" : ` → ${json(d.result, 40)}`}`;
      const error = d.error as { code?: string; message?: string } | undefined;
      return `${call()} failed${took}: ${error?.code ?? "error"}${error?.message ? `: ${error.message}` : ""}`;
    }
    default:
      return json(entry.data);
  }
}

/** `action.result` and friends are errors when the action failed; logs by level. */
export function toneOf(entry: JournalEntry): "good" | "warn" | "bad" | "muted" {
  const d = (entry.data ?? {}) as Data;
  switch (entry.kind) {
    case "action.result":
      return d.ok === true ? "good" : "bad";
    case "action.suppressed":
      return "warn";
    case "log":
      return d.level === "error" ? "bad" : d.level === "warn" ? "warn" : "muted";
    case "state.quality":
      return d.quality === "good" ? "good" : "warn";
    case "conn.status":
      return d.status === "ok" ? "good" : d.status === "error" ? "bad" : "warn";
    default:
      return "muted";
  }
}
