import { computed, onBeforeUnmount, shallowReactive, watch } from "vue";
import { api, ApiError } from "../api";
import type { Problem } from "../flows/model";
import { useLiveClient } from "../live/useLiveState";
import { useLiveStore } from "../stores/live";
import type { ControlBinding, Preview } from "./model";

export interface PreviewResult {
  preview?: Preview;
  /** Why there's no preview: the request failed. */
  error?: string;
  /** The binding didn't parse (`422`). */
  problems?: Problem[];
}

/**
 * Key previews from `POST /control-bindings/preview`, by control, kept up
 * to date: a binding is previewed again when it changes, and when state it
 * read last time changes. The paths it read are subscribed for that, so the
 * editor follows live state without polling.
 */
export function usePreviews(bindings: () => ControlBinding[], debounceMs = 150) {
  const client = useLiveClient();
  const store = useLiveStore();
  const results = shallowReactive(new Map<string, PreviewResult>());

  /** What each control was last previewed as, and the state it read then. */
  const asked = new Map<string, { json: string; sig: string; n: number }>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  const held = new Set<string>();
  let stopped = false;

  const readsOf = (control: string) => {
    const props = results.get(control)?.preview?.props ?? {};
    return [...new Set(Object.values(props).flatMap((p) => p?.reads ?? []))].sort();
  };
  const sigOf = (reads: string[]) =>
    JSON.stringify(reads.map((r) => store.leavesUnder(r).map((l) => [l.path, l.value, l.quality])));

  function hold() {
    const wanted = new Set([...results.keys()].flatMap(readsOf));
    for (const path of wanted) if (!held.has(path)) client.subscribe(path);
    for (const path of held) if (!wanted.has(path)) client.unsubscribe(path);
    held.clear();
    for (const path of wanted) held.add(path);
  }

  async function fetch(binding: ControlBinding) {
    const control = binding.control;
    const json = JSON.stringify(binding);
    const n = (asked.get(control)?.n ?? 0) + 1;
    asked.set(control, { json, sig: asked.get(control)?.sig ?? "", n });
    let result: PreviewResult;
    try {
      result = { preview: await api.previewBinding(binding) };
    } catch (e) {
      const found = e instanceof ApiError ? e.problems : null;
      result = found !== null ? { problems: found.errors } : { error: e instanceof Error ? e.message : String(e) };
    }
    if (stopped || asked.get(control)?.n !== n) return;
    results.set(control, result);
    asked.set(control, { json, sig: sigOf(readsOf(control)), n });
    hold();
  }

  function later(binding: ControlBinding, delay = debounceMs) {
    clearTimeout(timers.get(binding.control));
    timers.set(
      binding.control,
      setTimeout(() => {
        timers.delete(binding.control);
        void fetch(binding);
      }, delay),
    );
  }

  // A changed binding is previewed again; a gone one is forgotten.
  watch(
    () => bindings().map((b) => [b.control, JSON.stringify(b)] as const),
    (now) => {
      const current = new Map(now);
      for (const control of [...results.keys(), ...asked.keys()]) {
        if (current.has(control)) continue;
        results.delete(control);
        asked.delete(control);
        clearTimeout(timers.get(control));
      }
      for (const binding of bindings()) {
        const seen = asked.get(binding.control);
        if (seen?.json !== current.get(binding.control)) later(binding, seen === undefined ? 0 : debounceMs);
      }
      hold();
    },
    { immediate: true },
  );

  // State a binding read has changed: preview it again.
  const sigs = computed(() => new Map([...results.keys()].map((c) => [c, sigOf(readsOf(c))])));
  watch(sigs, (now) => {
    for (const binding of bindings()) {
      const seen = asked.get(binding.control);
      const sig = now.get(binding.control);
      if (seen !== undefined && sig !== undefined && sig !== seen.sig && !timers.has(binding.control)) later(binding);
    }
  });

  /** Previews `control` again now, e.g. after saving it. */
  function refresh(control: string) {
    const binding = bindings().find((b) => b.control === control);
    if (binding !== undefined) later(binding, 0);
  }

  onBeforeUnmount(() => {
    stopped = true;
    for (const timer of timers.values()) clearTimeout(timer);
    for (const path of held) client.unsubscribe(path);
    held.clear();
  });

  return { results, refresh };
}
