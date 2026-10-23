<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { api, ApiError } from "../api";
import LayerEditor from "../components/LayerEditor.vue";
import SurfacePanel from "../components/SurfacePanel.vue";
import { useLiveState } from "../live/useLiveState";
import { hrefOf, navigate } from "../nav";
import { useSurfacesStore } from "../stores/surfaces";
import { deckControls, missingControls, models, toLayerControl, VIRTUAL_DECK, virtualDeck, type ControlDef } from "../surfaces/model";

defineProps<{ surface?: string; layer?: string; control?: string }>();

const surfaces = useSurfacesStore();
const live = useLiveState("surface");

onMounted(() => void surfaces.refresh());

const value = (path: string) => live.leaves.value.find((l) => l.path === path)?.value;
const connected = (slug: string) => value(`surface.${slug}.connected`) === true;
const showing = (slug: string) => {
  const layer = value(`surface.${slug}.current.main`);
  return typeof layer === "string" ? (surfaces.layerBySlug(layer)?.name ?? layer) : null;
};

// ---- Creating ----

const deckName = ref("");
const deckError = ref<string | null>(null);
const layerName = ref("");
/** What a new layer copies its controls from: `surface:<id>`, or `model:<id>` with no surfaces yet. */
const layerFrom = ref("");
const from = computed({
  get: () =>
    layerFrom.value || (surfaces.surfaces[0] === undefined ? `model:${VIRTUAL_DECK}` : `surface:${surfaces.surfaces[0].id}`),
  set: (value: string) => (layerFrom.value = value),
});
const layerError = ref<string | null>(null);

function message(e: unknown): string {
  const found = e instanceof ApiError ? e.problems : null;
  if (found !== null && found.errors.length > 0) return found.errors.map((p) => p.message).join(" ");
  return e instanceof Error ? e.message : String(e);
}

async function createDeck() {
  deckError.value = null;
  // The first deck shows the first layer that fits it.
  const fits = surfaces.layers.find((l) => missingControls(l, deckControls(virtualDeck)).length === 0);
  try {
    const saved = await surfaces.saveSurface({
      id: "",
      name: deckName.value.trim() || "Virtual deck",
      slug: "",
      model: VIRTUAL_DECK,
      ...(fits === undefined ? {} : { startLayer: fits.id }),
    });
    deckName.value = "";
    navigate({ tab: "surfaces", surface: saved.surface.id });
  } catch (e) {
    deckError.value = message(e);
  }
}

/**
 * The controls a new layer copies (surfaces spec §1): what the surface last
 * attached with, or what its model attaches with if it hasn't attached since
 * the server started.
 */
async function controlsFrom(from: string): Promise<ControlDef[]> {
  const [kind, id] = [from.slice(0, from.indexOf(":")), from.slice(from.indexOf(":") + 1)];
  if (kind === "surface") {
    try {
      return await api.surfaceControls(id);
    } catch (e) {
      if (!(e instanceof ApiError && e.status === 404)) throw e;
    }
    const model = models[surfaces.surface(id)?.model ?? ""];
    if (model === undefined) throw new Error("That surface hasn't attached yet, so its controls aren't known.");
    return deckControls(model);
  }
  return deckControls(models[id] ?? virtualDeck);
}

async function createLayer() {
  layerError.value = null;
  try {
    const controls = (await controlsFrom(from.value)).map(toLayerControl);
    const saved = await surfaces.saveLayer({ id: "", name: layerName.value.trim(), slug: "", controls });
    layerName.value = "";
    navigate({ tab: "surfaces", layer: saved.layer.id });
  } catch (e) {
    layerError.value = message(e);
  }
}
</script>

<template>
  <LayerEditor
    v-if="layer !== undefined"
    :id="layer"
    :control="control"
  />
  <SurfacePanel
    v-else-if="surface !== undefined"
    :id="surface"
  />
  <template v-else>
    <section class="panel">
      <h2>Surfaces</h2>
      <p
        v-if="surfaces.error"
        class="error"
      >
        {{ surfaces.error }}
      </p>
      <p
        v-else-if="!surfaces.loaded"
        class="muted"
      >
        Loading…
      </p>
      <p
        v-else-if="surfaces.surfaces.length === 0"
        class="muted"
      >
        No surfaces yet. A virtual deck is a Stream Deck in this browser: it shows a layer and its keys fire flows.
      </p>
      <table
        v-else
        class="surfaces"
      >
        <thead>
          <tr>
            <th>Name</th>
            <th>Model</th>
            <th>Attached</th>
            <th>Showing</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="s in surfaces.surfaces"
            :key="s.id"
            :data-surface="s.slug"
          >
            <td>
              <a :href="hrefOf({ tab: 'surfaces', surface: s.id })">{{ s.name }}</a>
              <div class="muted">
                <code>surface.{{ s.slug }}</code>
              </div>
            </td>
            <td class="muted">
              {{ models[s.model]?.name ?? s.model }}
            </td>
            <td>
              <span
                class="badge"
                :class="connected(s.slug) ? 'good' : 'muted'"
              >{{ connected(s.slug) ? "Attached" : "Detached" }}</span>
            </td>
            <td>
              <span v-if="showing(s.slug)">{{ showing(s.slug) }}</span>
              <span
                v-else
                class="muted"
              >Nothing</span>
            </td>
          </tr>
        </tbody>
      </table>
      <form
        class="row create"
        @submit.prevent="createDeck"
      >
        <input
          v-model="deckName"
          placeholder="Virtual deck"
          aria-label="New deck name"
          autocomplete="off"
        >
        <button type="submit">
          New virtual deck
        </button>
        <span
          v-if="deckError"
          class="error"
          role="alert"
        >{{ deckError }}</span>
      </form>
    </section>

    <section class="panel">
      <h2>Layers</h2>
      <p
        v-if="surfaces.loaded && surfaces.layers.length === 0"
        class="muted"
      >
        No layers yet. A layer binds a set of controls to show state; any surface that has those controls can show it.
      </p>
      <table
        v-else-if="surfaces.layers.length > 0"
        class="layers"
      >
        <thead>
          <tr>
            <th>Name</th>
            <th>Controls</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="l in surfaces.layers"
            :key="l.id"
            :data-layer="l.slug"
          >
            <td>
              <a :href="hrefOf({ tab: 'surfaces', layer: l.id })">{{ l.name }}</a>
            </td>
            <td class="muted">
              {{ l.controls.length }}
            </td>
          </tr>
        </tbody>
      </table>
      <form
        class="row create"
        @submit.prevent="createLayer"
      >
        <input
          v-model="layerName"
          placeholder="Cameras"
          aria-label="New layer name"
          autocomplete="off"
          required
        >
        <label class="from">
          with the controls of
          <select
            v-model="from"
            aria-label="Copy controls from"
          >
            <option
              v-if="surfaces.surfaces.length === 0"
              :value="`model:${VIRTUAL_DECK}`"
            >
              {{ virtualDeck.name }}
            </option>
            <option
              v-for="s in surfaces.surfaces"
              :key="s.id"
              :value="`surface:${s.id}`"
            >
              {{ s.name }}
            </option>
          </select>
        </label>
        <button
          type="submit"
          :disabled="layerName.trim() === ''"
        >
          New layer
        </button>
        <span
          v-if="layerError"
          class="error"
          role="alert"
        >{{ layerError }}</span>
      </form>
    </section>
  </template>
</template>

<style scoped>
.create {
  margin-top: 0.75rem;
  gap: 0.5rem;
}

.from {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  color: var(--muted);
}
</style>
