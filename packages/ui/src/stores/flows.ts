import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { api } from "../api";
import type { Flow, FlowDraft, Saved } from "../flows/model";

/**
 * Flows over HTTP. They aren't on the WebSocket, so this refreshes after its
 * own saves; run status is live state under `flow` (flow spec §9).
 */
export const useFlowsStore = defineStore("flows", () => {
  const flows = ref<Flow[]>([]);
  const loaded = ref(false);
  const error = ref<string | null>(null);
  /** A draft for the next new flow to start from, e.g. a key's "When pressed". Taken once. */
  const prefill = ref<FlowDraft | null>(null);

  async function refresh() {
    try {
      flows.value = (await api.flows()).sort((a, b) => a.name.localeCompare(b.name));
      error.value = null;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loaded.value = true;
    }
  }

  /** Creates the flow when it has no ID, else replaces it. Throws `ApiError` 422 with problems. */
  async function save(flow: Flow): Promise<Saved> {
    const saved = flow.id === "" ? await api.createFlow(flow) : await api.updateFlow(flow);
    await refresh();
    return saved;
  }

  async function remove(id: string) {
    try {
      await api.deleteFlow(id);
    } finally {
      await refresh();
    }
  }

  const byId = computed(() => new Map(flows.value.map((f) => [f.id, f])));

  function get(id: string): Flow | undefined {
    return byId.value.get(id);
  }

  /** A flow's name, or its ID if it isn't known. */
  function name(id: string): string {
    return byId.value.get(id)?.name ?? id;
  }

  return { flows, loaded, error, prefill, refresh, save, remove, get, name };
});
