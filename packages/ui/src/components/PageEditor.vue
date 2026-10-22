<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { api, ApiError } from "../api";
import type { ControlBinding, RuntimeProblem } from "../bindings/model";
import { usePreviews } from "../bindings/usePreviews";
import { useLiveState } from "../live/useLiveState";
import { hrefOf, navigate } from "../nav";
import { useCatalogStore } from "../stores/catalog";
import { useFlowsStore } from "../stores/flows";
import { useSurfacesStore } from "../stores/surfaces";
import { controlsOf, type Page } from "../surfaces/model";
import BindingEditor from "./bindings/BindingEditor.vue";

/**
 * A page: its key grid, drawn by the core from live state, and the bindings
 * of the key that's selected. The page's name and size are below.
 */
const props = defineProps<{ id: string; control?: string }>();

const surfaces = useSurfacesStore();
const flows = useFlowsStore();
const catalog = useCatalogStore();
const page = computed(() => surfaces.page(props.id));

const bindings = ref<ControlBinding[]>([]);
const runtime = ref<RuntimeProblem[]>([]);
const loadError = ref<string | null>(null);

async function load() {
  try {
    const [b, r] = await Promise.all([api.bindings(props.id), api.bindingProblems()]);
    bindings.value = b;
    runtime.value = r;
    loadError.value = null;
  } catch (e) {
    loadError.value = e instanceof Error ? e.message : String(e);
  }
}

watch(() => props.id, load, { immediate: true });
// Bindings name connections by alias, and keys link to their flows.
onMounted(() => void Promise.all([flows.refresh(), catalog.refresh()]));

// Evaluation errors come and go with state; the editor checks now and then.
const problemTimer = setInterval(async () => {
  try {
    runtime.value = await api.bindingProblems();
  } catch {
    // Kept as they were; the next try may work.
  }
}, 5000);
onBeforeUnmount(() => clearInterval(problemTimer));

const byControl = computed(() => new Map(bindings.value.map((b) => [b.control, b])));
const { results } = usePreviews(() => bindings.value);
const controls = computed(() => (page.value === undefined ? [] : controlsOf(page.value.layout)));
const failing = (control: string) => {
  const id = byControl.value.get(control)?.id;
  return id !== undefined && runtime.value.some((p) => p.binding === id);
};
/** Flows with a gesture trigger on each key, for the grid's corner mark. */
const triggered = computed(() => {
  const out = new Set<string>();
  for (const f of flows.flows) {
    for (const t of f.triggers) if (t.kind === "gesture" && t.control.page === props.id) out.add(t.control.control);
  }
  return out;
});

function select(control: string) {
  navigate({ tab: "surfaces", page: props.id, control });
}

// Arrow keys move the selection around the grid.
function move(ev: KeyboardEvent) {
  const p = page.value;
  if (p === undefined || props.control === undefined) return;
  const n = Number(props.control.slice(1));
  const step: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -p.layout.cols, ArrowDown: p.layout.cols };
  const next = n + (step[ev.key] ?? 0);
  if (step[ev.key] === undefined || next < 0 || next >= controls.value.length) return;
  ev.preventDefault();
  select(`k${next}`);
  (ev.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-control="k${next}"]`)?.focus();
}

const runtimeFor = computed(() => {
  const id = props.control === undefined ? undefined : byControl.value.get(props.control)?.id;
  return runtime.value.filter((p) => p.binding === id);
});

// ---- Where it's shown ----

const live = useLiveState("surface");
const shownOn = computed(() =>
  surfaces.surfaces.filter((s) => {
    const leaf = live.leaves.value.find((l) => l.path === `surface.${s.slug}.page`);
    return page.value !== undefined && leaf?.value === page.value.slug;
  }),
);

// ---- Settings ----

const draft = ref<Page | null>(null);
const saveError = ref<string | null>(null);
const savedNote = ref<string | null>(null);

watch(
  [() => props.id, () => page.value !== undefined],
  () => {
    draft.value = page.value === undefined ? null : { ...page.value, layout: { ...page.value.layout } };
    saveError.value = null;
    savedNote.value = null;
  },
  { immediate: true },
);

async function saveSettings() {
  if (draft.value === null) return;
  saveError.value = null;
  savedNote.value = null;
  try {
    const saved = await surfaces.savePage({ ...draft.value, name: draft.value.name.trim() });
    draft.value = { ...saved.page, layout: { ...saved.page.layout } };
    savedNote.value = saved.warnings.length > 0 ? saved.warnings.map((w) => w.message).join(" ") : "Saved.";
  } catch (e) {
    const found = e instanceof ApiError ? e.problems : null;
    saveError.value = found !== null ? found.errors.map((p) => p.message).join(" ") : e instanceof Error ? e.message : String(e);
  }
}

async function remove() {
  const p = page.value;
  if (p === undefined) return;
  const n = bindings.value.length;
  const also = n === 0 ? "" : ` Its ${n} ${n === 1 ? "binding is" : "bindings are"} deleted too.`;
  if (!confirm(`Delete the page "${p.name}"?${also}`)) return;
  try {
    await surfaces.removePage(p.id);
    navigate({ tab: "surfaces" });
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <p>
    <a :href="hrefOf({ tab: 'surfaces' })">← Surfaces and pages</a>
  </p>
  <p
    v-if="page === undefined || draft === null"
    class="muted panel"
  >
    {{ surfaces.loaded ? "No such page." : "Loading…" }}
  </p>
  <template v-else>
    <section class="panel">
      <div class="row title">
        <h2>{{ page.name }}</h2>
        <span class="muted">
          <template v-if="shownOn.length > 0">
            Showing on
            <template
              v-for="(s, i) in shownOn"
              :key="s.id"
            >{{ i > 0 ? ", " : "" }}<a :href="hrefOf({ tab: 'surfaces', surface: s.id })">{{ s.name }}</a></template>
          </template>
          <template v-else>Not on any surface right now</template>
        </span>
      </div>
      <p
        v-if="loadError"
        class="error"
      >
        {{ loadError }}
      </p>
      <div class="editor">
        <div
          class="grid"
          :style="{ gridTemplateColumns: `repeat(${page.layout.cols}, var(--key))` }"
          role="listbox"
          aria-label="Keys"
          @keydown="move"
        >
          <button
            v-for="c in controls"
            :key="c"
            type="button"
            class="key"
            role="option"
            :aria-selected="c === control"
            :aria-label="`Key ${c}${byControl.has(c) ? ', bound' : ''}`"
            :class="{ selected: c === control, failing: failing(c) }"
            :data-control="c"
            @click="select(c)"
          >
            <img
              v-if="results.get(c)?.preview"
              :src="`data:image/png;base64,${results.get(c)!.preview!.image}`"
              alt=""
            >
            <span
              v-else
              class="key-id"
            >{{ c }}</span>
            <span
              v-if="triggered.has(c)"
              class="mark"
              title="A flow triggers on this key"
            />
          </button>
        </div>
        <div class="selected-key">
          <BindingEditor
            v-if="control !== undefined && controls.includes(control)"
            :key="`${page.id}/${control}`"
            :page="page"
            :control="control"
            :stored="byControl.get(control)"
            :runtime="runtimeFor"
            @saved="load"
          />
          <p
            v-else
            class="muted"
          >
            Pick a key to bind what it shows: a label, a text colour and a background, each from live state.
          </p>
        </div>
      </div>
    </section>

    <form
      class="panel"
      novalidate
      @submit.prevent="saveSettings"
    >
      <h3>Page</h3>
      <div class="row">
        <div class="field">
          <label for="page-name">Name</label>
          <input
            id="page-name"
            v-model="draft.name"
            autocomplete="off"
          >
        </div>
        <div class="field">
          <label for="page-cols">Size</label>
          <span class="row size">
            <input
              id="page-cols"
              v-model.number="draft.layout.cols"
              type="number"
              min="1"
              max="32"
              aria-label="Columns"
            >
            ×
            <input
              v-model.number="draft.layout.rows"
              type="number"
              min="1"
              max="32"
              aria-label="Rows"
            >
          </span>
          <small class="hint">A surface shows it only if it's the same size.</small>
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
.title {
  justify-content: space-between;
  margin-bottom: 0.75rem;
}

.title h2 {
  margin: 0;
}

.editor {
  --key: 64px;
  display: flex;
  flex-wrap: wrap;
  gap: 1.5rem;
  align-items: flex-start;
}

.grid {
  display: inline-grid;
  gap: 8px;
  padding: 12px;
  border-radius: 14px;
  background: #0b0b0c;
  position: sticky;
  top: 1rem;
}

.key {
  position: relative;
  width: var(--key);
  height: var(--key);
  padding: 0;
  border: none;
  border-radius: 8px;
  overflow: hidden;
  background: #000;
  box-shadow: 0 0 0 1px #26272b;
}

.key img {
  display: block;
  width: 100%;
  height: 100%;
}

.key.selected {
  box-shadow: 0 0 0 3px var(--accent);
}

.key.failing::after {
  content: "";
  position: absolute;
  inset: 0;
  border: 2px solid #fc8181;
  border-radius: 8px;
}

.key:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.key-id {
  color: #5d6068;
  font-size: 0.8em;
}

.mark {
  position: absolute;
  left: 5px;
  bottom: 5px;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #6b9cf0;
}

.selected-key {
  flex: 1;
  min-width: min(100%, 26rem);
}

.size input {
  width: 4.5rem;
}

button.danger {
  border-color: var(--bad);
  color: var(--bad);
}
</style>
