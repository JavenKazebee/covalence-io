<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { api, ApiError } from "../api";
import { useLiveState } from "../live/useLiveState";
import { hrefOf, navigate } from "../nav";
import { useSurfacesStore } from "../stores/surfaces";
import { deckControls, missingControls, models, type Surface } from "../surfaces/model";
import VirtualDeck from "./VirtualDeck.vue";

/**
 * One surface: its virtual deck, attached while this is open, the layer it
 * shows, and its settings.
 */
const props = defineProps<{ id: string }>();

const surfaces = useSurfacesStore();
const stored = computed(() => surfaces.surface(props.id));
const slug = computed(() => stored.value?.slug ?? "");
const live = useLiveState(() => (slug.value === "" ? "" : `surface.${slug.value}`));

const value = (field: string) => live.leaves.value.find((l) => l.path === `surface.${slug.value}.${field}`)?.value;
/** The layer shown now, from `surface.<slug>.current.main`, which holds its slug. */
const shown = computed(() => {
  const layer = value("current.main");
  return typeof layer === "string" ? surfaces.layerBySlug(layer) : undefined;
});
/** The controls the virtual deck attaches with. */
const controls = computed(() => {
  const model = stored.value === undefined ? undefined : models[stored.value.model];
  return model === undefined ? undefined : deckControls(model);
});
/** Layers this surface can show: it has each of their controls (surfaces spec §5.4). */
const fitting = computed(() =>
  surfaces.layers.filter((l) => controls.value !== undefined && missingControls(l, controls.value).length === 0),
);

const showError = ref<string | null>(null);
const switching = ref(false);

async function show(layer: string) {
  showError.value = null;
  switching.value = true;
  try {
    await api.navigate(props.id, layer === "" ? null : layer);
  } catch (e) {
    showError.value = e instanceof Error ? e.message : String(e);
  } finally {
    switching.value = false;
  }
}

// ---- Settings ----

const draft = ref<Surface | null>(null);
const saveError = ref<string | null>(null);
const savedNote = ref<string | null>(null);

watch(
  [() => props.id, () => stored.value !== undefined],
  () => {
    draft.value = stored.value === undefined ? null : { ...stored.value };
    saveError.value = null;
    savedNote.value = null;
  },
  { immediate: true },
);

async function save() {
  if (draft.value === null) return;
  saveError.value = null;
  savedNote.value = null;
  const surface = { ...draft.value, name: draft.value.name.trim() };
  if (!surface.startLayer) delete surface.startLayer;
  try {
    const saved = await surfaces.saveSurface(surface);
    draft.value = { ...saved.surface };
    savedNote.value = "Saved.";
  } catch (e) {
    const found = e instanceof ApiError ? e.problems : null;
    saveError.value = found !== null ? found.errors.map((p) => p.message).join(" ") : e instanceof Error ? e.message : String(e);
  }
}

async function remove() {
  if (stored.value === undefined) return;
  if (!confirm(`Delete the surface "${stored.value.name}"? It detaches, and its state is deleted.`)) return;
  try {
    await surfaces.removeSurface(props.id);
    navigate({ tab: "surfaces" });
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <p>
    <a :href="hrefOf({ tab: 'surfaces' })">← All surfaces</a>
  </p>
  <p
    v-if="stored === undefined || draft === null"
    class="muted panel"
  >
    {{ surfaces.loaded ? "No such surface." : "Loading…" }}
  </p>
  <template v-else>
    <section class="panel surface">
      <h2>{{ stored.name }}</h2>
      <div class="layout">
        <VirtualDeck :surface="stored" />
        <div class="side">
          <div class="field">
            <label for="surface-layer">Showing</label>
            <select
              id="surface-layer"
              :value="shown?.id ?? ''"
              :disabled="switching"
              @change="show(($event.target as HTMLSelectElement).value)"
            >
              <option value="">
                Nothing
              </option>
              <option
                v-for="p in fitting"
                :key="p.id"
                :value="p.id"
              >
                {{ p.name }}
              </option>
            </select>
            <small
              v-if="fitting.length === 0"
              class="hint"
            >No layers fit it yet.
              <a :href="hrefOf({ tab: 'surfaces' })">Make one</a> from it.</small>
            <small
              v-if="showError"
              class="error"
              role="alert"
            >{{ showError }}</small>
          </div>
          <p v-if="shown">
            <a
              class="button"
              :href="hrefOf({ tab: 'surfaces', layer: shown.id })"
            >Edit {{ shown.name }}</a>
          </p>
          <p class="hint">
            Press keys with the mouse, or focus one and hold Space. Each press is a
            <code>gesture</code> a flow can trigger on; the keys show what their layer binds.
          </p>
        </div>
      </div>
    </section>

    <form
      class="panel"
      novalidate
      @submit.prevent="save"
    >
      <h3>Settings</h3>
      <div class="row">
        <div class="field">
          <label for="surface-name">Name</label>
          <input
            id="surface-name"
            v-model="draft.name"
            autocomplete="off"
          >
          <small class="hint">Its state is at <code>surface.{{ stored.slug }}</code>.</small>
        </div>
        <div class="field">
          <label for="surface-start">Start layer</label>
          <select
            id="surface-start"
            v-model="draft.startLayer"
          >
            <option :value="undefined">
              None
            </option>
            <option
              v-for="p in fitting"
              :key="p.id"
              :value="p.id"
            >
              {{ p.name }}
            </option>
          </select>
          <small class="hint">Shown the first time it attaches. After that it shows the layer it last showed.</small>
        </div>
      </div>
      <div class="row">
        <button type="submit">
          Save
        </button>
        <button
          type="button"
          class="secondary danger"
          @click="remove"
        >
          Delete
        </button>
        <span
          v-if="saveError"
          class="error"
          role="alert"
        >{{ saveError }}</span>
        <span
          v-else-if="savedNote"
          class="muted"
          role="status"
        >{{ savedNote }}</span>
      </div>
    </form>
  </template>
</template>

<style scoped>
.layout {
  display: flex;
  flex-wrap: wrap;
  gap: 1.5rem;
  align-items: flex-start;
}

.side {
  flex: 1;
  min-width: 14rem;
}

a.button {
  display: inline-block;
  padding: 0.4rem 0.9rem;
  border-radius: 6px;
  border: 1px solid var(--accent);
  color: var(--accent);
  text-decoration: none;
}

button.danger {
  border-color: var(--bad);
  color: var(--bad);
}
</style>
