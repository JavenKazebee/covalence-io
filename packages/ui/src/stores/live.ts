import { defineStore } from "pinia";
import { ref, shallowReactive } from "vue";
import type { HostStatus, Leaf } from "../live/protocol";
import { comparePaths, isUnder, isUnderAny } from "../live/paths";

/**
 * Live state from `/ws`, keyed by text path. Fed only by the shared client
 * (`live/client.ts`); components read it through `useLiveState`.
 */
export const useLiveStore = defineStore("live", () => {
  const leaves = shallowReactive(new Map<string, Leaf>());
  /** Whether the WebSocket is open. Held leaves may be out of date while it isn't. */
  const connected = ref(false);
  const host = ref<HostStatus | null>(null);
  /** The last `error` message from the server. */
  const lastError = ref<string | null>(null);

  function leaf(path: string): Leaf | undefined {
    return leaves.get(path);
  }

  /** Every held leaf at or below `prefix`, in path order. */
  function leavesUnder(prefix: string): Leaf[] {
    const out: Leaf[] = [];
    for (const [path, leaf] of leaves) {
      if (isUnder(path, prefix)) out.push(leaf);
    }
    return out.sort((a, b) => comparePaths(a.path, b.path));
  }

  /** A snapshot replaces everything held under its paths. */
  function applySnapshot(paths: string[], snapshot: Leaf[]) {
    if (paths.length === 0) return;
    for (const path of leaves.keys()) {
      if (isUnderAny(path, paths)) leaves.delete(path);
    }
    for (const leaf of snapshot) leaves.set(leaf.path, leaf);
  }

  function applyPatch(set: Leaf[], deleted: string[]) {
    for (const leaf of set) leaves.set(leaf.path, leaf);
    for (const path of deleted) leaves.delete(path);
  }

  /** Drops leaves under `removed` that no prefix in `kept` still covers. */
  function prune(removed: string[], kept: Iterable<string>) {
    const keep = [...kept];
    for (const path of leaves.keys()) {
      if (isUnderAny(path, removed) && !isUnderAny(path, keep)) leaves.delete(path);
    }
  }

  return {
    leaves,
    connected,
    host,
    lastError,
    leaf,
    leavesUnder,
    applySnapshot,
    applyPatch,
    prune,
  };
});

export type LiveStore = ReturnType<typeof useLiveStore>;
