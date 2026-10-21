import { computed, onBeforeUnmount, onMounted, ref, shallowRef, triggerRef, watch } from "vue";
import { api } from "../api";
import type { JournalEntry } from "../live/protocol";
import { useLiveClient } from "../live/useLiveState";
import { useLiveStore } from "../stores/live";

/** Entries kept on screen. Older ones are still in `GET /journal`. */
export const KEEP = 500;
const PAGE = 1000;

/**
 * The newest journal entries, live. Follows the tail first, then fetches the
 * backlog, and dedupes by `seq` (state spec §1.6). Gaps — a `lagged` tail or a
 * reconnect — are refetched over HTTP.
 */
export function useJournal() {
  const client = useLiveClient();
  const live = useLiveStore();
  const bySeq = shallowRef(new Map<number, JournalEntry>());
  const error = ref<string | null>(null);
  const loading = ref(true);
  let maxSeq = 0;

  function add(entries: Iterable<JournalEntry>) {
    const map = bySeq.value;
    for (const entry of entries) {
      map.set(entry.seq, entry);
      if (entry.seq > maxSeq) maxSeq = entry.seq;
    }
    if (map.size > KEEP) {
      const drop = [...map.keys()].sort((a, b) => a - b).slice(0, map.size - KEEP);
      for (const seq of drop) map.delete(seq);
    }
    triggerRef(bySeq);
  }

  async function fetchFrom(from: number, limit: number) {
    try {
      add(await api.journal(Math.max(1, from), Math.min(limit, PAGE)));
      error.value = null;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  async function backlog() {
    loading.value = true;
    try {
      const { journal } = await api.health();
      await fetchFrom(journal.lastSeq - KEEP + 1, KEEP);
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  let stop: (() => void) | null = null;
  onMounted(() => {
    stop = client.followJournal({
      entry: (entry) => add([entry]),
      // Entries arrive in order, so the gap starts just after the newest held.
      lagged: (missed) => void fetchFrom(maxSeq + 1, missed),
    });
    void backlog();
  });
  onBeforeUnmount(() => stop?.());

  // The tail missed whatever happened while the socket was closed.
  watch(
    () => live.connected,
    (connected, was) => {
      if (connected && was === false && maxSeq > 0) void fetchFrom(maxSeq + 1, PAGE);
    },
  );

  return {
    /** Newest first. */
    entries: computed(() => [...bySeq.value.values()].sort((a, b) => b.seq - a.seq)),
    get: (seq: number) => bySeq.value.get(seq),
    error,
    loading,
  };
}
