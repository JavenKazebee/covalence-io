<script setup lang="ts">
import { onMounted, ref } from "vue";
import { ApiError } from "../api";
import PageEditor from "../components/PageEditor.vue";
import SurfacePanel from "../components/SurfacePanel.vue";
import { useLiveState } from "../live/useLiveState";
import { hrefOf, navigate } from "../nav";
import { useSurfacesStore } from "../stores/surfaces";
import { layoutText, models, VIRTUAL_DECK } from "../surfaces/model";

defineProps<{ surface?: string; page?: string; control?: string }>();

const surfaces = useSurfacesStore();
const live = useLiveState("surface");

onMounted(() => void surfaces.refresh());

const value = (path: string) => live.leaves.value.find((l) => l.path === path)?.value;
const connected = (slug: string) => value(`surface.${slug}.connected`) === true;
const showing = (slug: string) => {
  const page = value(`surface.${slug}.page`);
  return typeof page === "string" ? (surfaces.pageBySlug(page)?.name ?? page) : null;
};

// ---- Creating ----

const deckName = ref("");
const deckError = ref<string | null>(null);
const pageName = ref("");
const pageCols = ref(5);
const pageRows = ref(3);
const pageError = ref<string | null>(null);

function message(e: unknown): string {
  const found = e instanceof ApiError ? e.problems : null;
  if (found !== null && found.errors.length > 0) return found.errors.map((p) => p.message).join(" ");
  return e instanceof Error ? e.message : String(e);
}

async function createDeck() {
  deckError.value = null;
  // The first deck shows the first page that fits it.
  const fits = surfaces.pages.find((p) => p.layout.cols === 5 && p.layout.rows === 3);
  try {
    const saved = await surfaces.saveSurface({
      id: "",
      name: deckName.value.trim() || "Virtual deck",
      slug: "",
      model: VIRTUAL_DECK,
      ...(fits === undefined ? {} : { startPage: fits.id }),
    });
    deckName.value = "";
    navigate({ tab: "surfaces", surface: saved.surface.id });
  } catch (e) {
    deckError.value = message(e);
  }
}

async function createPage() {
  pageError.value = null;
  try {
    const saved = await surfaces.savePage({
      id: "",
      name: pageName.value.trim(),
      slug: "",
      layout: { cols: Number(pageCols.value), rows: Number(pageRows.value) },
    });
    pageName.value = "";
    navigate({ tab: "surfaces", page: saved.page.id });
  } catch (e) {
    pageError.value = message(e);
  }
}
</script>

<template>
  <PageEditor
    v-if="page !== undefined"
    :id="page"
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
      <h2>Pages</h2>
      <p
        v-if="surfaces.loaded && surfaces.pages.length === 0"
        class="muted"
      >
        No pages yet. A page is a grid of keys, each bound to show state; any surface of its size can show it.
      </p>
      <table
        v-else-if="surfaces.pages.length > 0"
        class="pages"
      >
        <thead>
          <tr>
            <th>Name</th>
            <th>Size</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="p in surfaces.pages"
            :key="p.id"
            :data-page="p.slug"
          >
            <td>
              <a :href="hrefOf({ tab: 'surfaces', page: p.id })">{{ p.name }}</a>
            </td>
            <td class="muted">
              {{ layoutText(p.layout) }}
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
        <label class="size">
          <input
            v-model.number="pageCols"
            type="number"
            min="1"
            max="32"
            aria-label="Columns"
          >
          ×
          <input
            v-model.number="pageRows"
            type="number"
            min="1"
            max="32"
            aria-label="Rows"
          >
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

.size {
  display: flex;
  align-items: center;
  gap: 0.3rem;
  color: var(--muted);
}

.size input {
  width: 4rem;
}
</style>
