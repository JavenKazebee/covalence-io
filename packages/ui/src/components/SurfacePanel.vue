<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { api, ApiError } from "../api";
import { useLiveState } from "../live/useLiveState";
import { hrefOf, navigate } from "../nav";
import { useCatalogStore } from "../stores/catalog";
import { useSurfacesStore } from "../stores/surfaces";
import { MAX_VIRTUAL, missingControls, surfaceControls, type ControlDef, type Surface } from "../surfaces/model";
import VirtualDeck from "./VirtualDeck.vue";

/**
 * One surface: its virtual deck, attached while this is open, or the
 * connection that provides it; the page it shows, and its settings.
 */
const props = defineProps<{ id: string }>();

const surfaces = useSurfacesStore();
const catalog = useCatalogStore();
const stored = computed(() => surfaces.surface(props.id));
const slug = computed(() => stored.value?.slug ?? "");
const live = useLiveState(() => (slug.value === "" ? "" : `surface.${slug.value}`));

const value = (field: string) => live.leaves.value.find((l) => l.path === `surface.${slug.value}.${field}`)?.value;
/** The page shown now, from `surface.<slug>.current.main`, which holds its slug. */
const shown = computed(() => {
  const current = value("current.main");
  return typeof current === "string" ? surfaces.componentBySlug(current) : undefined;
});
/** Whether a plugin's device is attached as this surface, from `surface.<slug>.connected`. */
const attached = computed(() => value("connected") === true);

/** What a plugin's surface last attached with; the server knows only once it has attached. */
const attachedControls = ref<ControlDef[] | undefined>(undefined);
watch(
  [() => props.id, () => stored.value?.provider, attached],
  async ([id, provider]) => {
    if (provider === undefined) return;
    try {
      const got = await api.surfaceControls(id);
      if (id === props.id) attachedControls.value = got;
    } catch {
      // 404 until it has attached since the server started.
      if (id === props.id) attachedControls.value = undefined;
    }
  },
  { immediate: true },
);

/** The controls the surface has: the virtual deck's, or what the plugin's device attached with. */
const controls = computed(() => {
  if (stored.value?.provider !== undefined) return attachedControls.value;
  return stored.value === undefined ? undefined : surfaceControls(stored.value);
});
/** Pages this surface can show: it has each of their controls (surfaces spec §5.4). */
const fitting = computed(() =>
  surfaces.shown.filter((l) => controls.value !== undefined && missingControls(l, controls.value).length === 0),
);

const showError = ref<string | null>(null);
const switching = ref(false);

async function show(component: string) {
  showError.value = null;
  switching.value = true;
  try {
    await api.navigate(props.id, component === "" ? null : component);
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

/** A virtual layout field, for the settings form: 0 encoders and faders with a motor is none. */
function setVirtual(field: "encoders" | "faders" | "motor", value: number | boolean) {
  if (draft.value === null) return;
  const next = { ...draft.value.virtual, [field]: value };
  draft.value.virtual = next;
}

async function save() {
  if (draft.value === null) return;
  saveError.value = null;
  savedNote.value = null;
  const surface = { ...draft.value, name: draft.value.name.trim() };
  if (!surface.startComponent) delete surface.startComponent;
  const v = surface.virtual;
  if (v !== undefined && !v.encoders && !v.faders && v.motor !== false) delete surface.virtual;
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
        <div
          v-if="stored.provider !== undefined"
          class="provided"
        >
          <p>
            <span
              class="badge"
              :class="attached ? 'good' : ''"
            >{{ attached ? "Attached" : "Detached" }}</span>
          </p>
          <p>
            Provided by
            <a :href="hrefOf({ tab: 'connections', conn: catalog.alias(stored.provider) })"><code>{{ catalog.alias(stored.provider) }}</code></a>
          </p>
          <p class="muted">
            <code>{{ stored.model }}</code><template v-if="stored.serial">
              · serial <code>{{ stored.serial }}</code>
            </template>
          </p>
          <p
            v-if="controls"
            class="muted"
          >
            {{ controls.length }} controls
          </p>
        </div>
        <VirtualDeck
          v-else
          :surface="stored"
        />
        <div class="side">
          <div class="field">
            <label for="surface-page">Showing</label>
            <select
              id="surface-page"
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
              v-if="controls === undefined"
              class="hint"
            >Its controls are known once it has attached.</small>
            <small
              v-else-if="fitting.length === 0"
              class="hint"
            >No pages fit it yet.
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
              :href="hrefOf({ tab: 'surfaces', component: shown.id })"
            >Edit {{ shown.name }}</a>
          </p>
          <p
            v-if="stored.provider === undefined"
            class="hint"
          >
            Press keys with the mouse, or focus one and hold Space. Each press is a
            <code>control</code> event a flow can trigger on; the keys show what their page binds.
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
          <label for="surface-start">Start page</label>
          <select
            id="surface-start"
            v-model="draft.startComponent"
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
          <small class="hint">Shown the first time it attaches. After that it shows the page it last showed.</small>
        </div>
      </div>
      <div
        v-if="stored.provider === undefined"
        class="row"
      >
        <div class="field">
          <label for="surface-encoders">Encoders</label>
          <select
            id="surface-encoders"
            :value="draft.virtual?.encoders ?? 0"
            @change="setVirtual('encoders', Number(($event.target as HTMLSelectElement).value))"
          >
            <option
              v-for="n in MAX_VIRTUAL + 1"
              :key="n"
              :value="n - 1"
            >
              {{ n - 1 }}
            </option>
          </select>
          <small class="hint"><code>e0</code>… beneath the keys: they turn and press.</small>
        </div>
        <div class="field">
          <label for="surface-faders">Faders</label>
          <select
            id="surface-faders"
            :value="draft.virtual?.faders ?? 0"
            @change="setVirtual('faders', Number(($event.target as HTMLSelectElement).value))"
          >
            <option
              v-for="n in MAX_VIRTUAL + 1"
              :key="n"
              :value="n - 1"
            >
              {{ n - 1 }}
            </option>
          </select>
          <small class="hint"><code>f0</code>… beside the keys: they move and sense touch.</small>
        </div>
        <div class="field">
          <label>
            <input
              type="checkbox"
              :checked="draft.virtual?.motor ?? true"
              @change="setVirtual('motor', ($event.target as HTMLInputElement).checked)"
            >
            Motorised faders
          </label>
          <small class="hint">A motorised fader moves to the value bound to it.</small>
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

.provided {
  min-width: 14rem;
}

.provided p {
  margin: 0 0 0.5rem;
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
