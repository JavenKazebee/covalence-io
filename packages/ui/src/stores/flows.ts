import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import { api } from "../api";
import { isInstance, type Flow, type FlowDraft, type FlowInstance, type Saved } from "../flows/model";

/**
 * Flows over HTTP. They aren't on the WebSocket, so this refreshes after its
 * own saves; run status is live state under `flow` (flow spec §9).
 */
export const useFlowsStore = defineStore("flows", () => {
  // Replaced whole on each refresh; shallow, as their sources nest deeply.
  const flows = shallowRef<Flow[]>([]);
  /** Instances of flow templates (composition §9), which run their template's steps. */
  const instances = shallowRef<FlowInstance[]>([]);
  const loaded = ref(false);
  const error = ref<string | null>(null);
  /** A draft for the next new flow to start from, e.g. a key's "When pressed". Taken once. */
  const prefill = shallowRef<FlowDraft | null>(null);

  async function refresh() {
    try {
      const all = (await api.flows()).sort((a, b) => a.name.localeCompare(b.name));
      flows.value = all.filter((f): f is Flow => !isInstance(f));
      instances.value = all.filter(isInstance);
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

  /** A flow's name, an instance's included, or its ID if it isn't known. */
  function name(id: string): string {
    return byId.value.get(id)?.name ?? instances.value.find((i) => i.id === id)?.name ?? id;
  }

  return { flows, instances, loaded, error, prefill, refresh, save, remove, get, name };
});
