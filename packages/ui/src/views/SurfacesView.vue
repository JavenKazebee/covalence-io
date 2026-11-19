<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { api, ApiError } from "../api";
import ComponentEditor from "../components/ComponentEditor.vue";
import SurfacePanel from "../components/SurfacePanel.vue";
import { useLiveState } from "../live/useLiveState";
import { hrefOf, navigate } from "../nav";
import { useCatalogStore } from "../stores/catalog";
import { useSurfacesStore } from "../stores/surfaces";
import { deckControls, missingControls, models, surfaceControls, toControlSpec, VIRTUAL_DECK, virtualDeck, type ControlDef } from "../surfaces/model";

defineProps<{ surface?: string; component?: string; control?: string }>();

const surfaces = useSurfacesStore();
const catalog = useCatalogStore();
const live = useLiveState("surface");

onMounted(() => void surfaces.refresh());

const value = (path: string) => live.leaves.value.find((l) => l.path === path)?.value;
const connected = (slug: string) => value(`surface.${slug}.connected`) === true;
const showing = (slug: string) => {
  const current = value(`surface.${slug}.current.main`);
  return typeof current === "string" ? (surfaces.componentBySlug(current)?.name ?? current) : null;
};

// ---- Creating ----

const deckName = ref("");
const deckError = ref<string | null>(null);
const pageName = ref("");
/** What a new page copies its controls from: `surface:<id>`, or `model:<id>` with no surfaces yet. */
const pageFrom = ref("");
const from = computed({
  get: () =>
    pageFrom.value || (surfaces.surfaces[0] === undefined ? `model:${VIRTUAL_DECK}` : `surface:${surfaces.surfaces[0].id}`),
  set: (value: string) => (pageFrom.value = value),
});
const pageError = ref<string | null>(null);

function message(e: unknown): string {
  const found = e instanceof ApiError ? e.problems : null;
  if (found !== null && found.errors.length > 0) return found.errors.map((p) => p.message).join(" ");
  return e instanceof Error ? e.message : String(e);
}

async function createDeck() {
  deckError.value = null;
  // The first deck shows the first page that fits it.
  const fits = surfaces.shown.find((l) => missingControls(l, deckControls(virtualDeck)).length === 0);
  try {
    const saved = await surfaces.saveSurface({
      id: "",
      name: deckName.value.trim() || "Virtual deck",
      slug: "",
      model: VIRTUAL_DECK,
      ...(fits === undefined ? {} : { startComponent: fits.id }),
    });
    deckName.value = "";
    navigate({ tab: "surfaces", surface: saved.surface.id });
  } catch (e) {
    deckError.value = message(e);
  }
}

/**
 * The controls a new page copies (surfaces spec §1): what the surface last
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
    const surface = surfaces.surface(id);
    const controls = surface === undefined ? undefined : surfaceControls(surface);
    if (controls === undefined) throw new Error("That surface hasn't attached yet, so its controls aren't known.");
    return controls;
  }
  return deckControls(models[id] ?? virtualDeck);
}

async function createPage() {
  pageError.value = null;
  try {
    const controls = (await controlsFrom(from.value)).map(toControlSpec);
    const saved = await surfaces.saveComponent({ id: "", name: pageName.value.trim(), slug: "", controls, show: {} });
    pageName.value = "";
    navigate({ tab: "surfaces", component: saved.component.id });
  } catch (e) {
    pageError.value = message(e);
  }
}
</script>

<template>
  <ComponentEditor
    v-if="component !== undefined"
    :id="component"
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
        No surfaces yet. A virtual deck is a Stream Deck in this browser: it shows a page and its keys fire flows.
      </p>
      <table
        v-else
        class="surfaces"
      >
        <thead>
          <tr>
            <th>Name</th>
            <th>Model</th>
            <th>Provider</th>
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
              <a
                v-if="s.provider !== undefined"
                :href="hrefOf({ tab: 'connections', conn: catalog.alias(s.provider) })"
              ><code>{{ catalog.alias(s.provider) }}</code></a>
              <span
                v-else
                class="muted"
              >This browser</span>
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
      <h2>Pages</h2>
      <p
        v-if="surfaces.loaded && surfaces.shown.length === 0"
        class="muted"
      >
        No pages yet. A page binds a set of controls to show state; any surface that has those controls can show it.
      </p>
      <table
        v-else-if="surfaces.shown.length > 0"
        class="pages"
      >
        <thead>
          <tr>
            <th>Name</th>
            <th>Controls</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="l in surfaces.shown"
            :key="l.id"
            :data-component="l.slug"
          >
            <td>
              <a :href="hrefOf({ tab: 'surfaces', component: l.id })">{{ l.name }}</a>
            </td>
            <td class="muted">
              {{ l.controls.length }}
            </td>
          </tr>
        </tbody>
      </table>
      <form
        class="row create"
        @submit.prevent="createPage"
      >
        <input
          v-model="pageName"
          placeholder="Cameras"
          aria-label="New page name"
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
          :disabled="pageName.trim() === ''"
        >
          New page
        </button>
        <span
          v-if="pageError"
          class="error"
          role="alert"
        >{{ pageError }}</span>
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
