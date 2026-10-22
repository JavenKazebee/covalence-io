import { splitPath } from "../live/paths";
import type { Leaf } from "../live/protocol";
import type { LastRun } from "./model";

export interface FlowStatus {
  running: number;
  lastRun: LastRun | null;
}

/**
 * Status by slug from the leaves under `flow` (flow spec §5.4):
 * `flow.<slug>.running` and `flow.<slug>.lastRun.{status, startedAt, durationMs, error?}`.
 */
export function flowStatuses(leaves: readonly Leaf[]): Map<string, FlowStatus> {
  const out = new Map<string, FlowStatus>();
  for (const leaf of leaves) {
    const { root, segments } = splitPath(leaf.path);
    const [slug, field, ...rest] = segments;
    if (root !== "flow" || typeof slug !== "string") continue;
    const status = out.get(slug) ?? { running: 0, lastRun: null };
    out.set(slug, status);
    if (field === "running" && rest.length === 0) {
      status.running = typeof leaf.value === "number" ? leaf.value : 0;
    } else if (field === "lastRun") {
      status.lastRun ??= {};
      setIn(status.lastRun as Record<string, unknown>, rest, leaf.value);
    }
  }
  return out;
}

/** Puts `value` at `path` in `target`, so a leaf per field or one object leaf both read the same. */
function setIn(target: Record<string, unknown>, path: readonly (string | number)[], value: unknown) {
  if (path.length === 0) {
    if (value !== null && typeof value === "object") Object.assign(target, value);
    return;
  }
  let at = target;
  for (const segment of path.slice(0, -1)) {
    const next = at[segment];
    at = (next !== null && typeof next === "object" ? next : (at[segment] = {})) as Record<string, unknown>;
  }
  at[path[path.length - 1]!] = value;
}
