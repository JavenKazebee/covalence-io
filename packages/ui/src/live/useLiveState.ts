import { computed, onBeforeUnmount, onMounted, toValue, watch, type MaybeRefOrGetter } from "vue";
import { useLiveStore } from "../stores/live";
import { LiveClient } from "./client";

let shared: LiveClient | null = null;

/** The app's one live client, started on first use. */
export function useLiveClient(): LiveClient {
  if (shared === null) {
    shared = new LiveClient(useLiveStore());
    shared.start();
  }
  return shared;
}

/**
 * Live leaves at or below `path`. Subscribes on mount and unsubscribes on
 * unmount; a changed `path` moves the subscription, and an empty one drops it.
 */
export function useLiveState(path: MaybeRefOrGetter<string>) {
  const store = useLiveStore();
  const client = useLiveClient();
  let held: string | null = null;

  function hold(next: string | null) {
    if (next === held) return;
    // Subscribe first, so leaves both paths cover aren't dropped in between.
    if (next !== null) client.subscribe(next);
    if (held !== null) client.unsubscribe(held);
    held = next;
  }

  // An empty path holds nothing, e.g. before there's a connection to show.
  const wanted = () => toValue(path) || null;
  onMounted(() => {
    hold(wanted());
    watch(wanted, hold);
  });
  onBeforeUnmount(() => hold(null));

  return {
    /** Every leaf at or below `path`, in path order. */
    leaves: computed(() => store.leavesUnder(toValue(path))),
    /** The leaf at exactly `path`, if `path` names one. */
    leaf: computed(() => store.leaf(toValue(path))),
  };
}
