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
import { gridOf, neighbour, type Layer } from "../surfaces/model";
import BindingEditor from "./bindings/BindingEditor.vue";

/**
 * A layer: its controls where they are on the surface, keys drawn by the core
 * from live state, and the bindings of the control that's selected. The
 * layer's name is below.
 */
const props = defineProps<{ id: string; control?: string }>();

const surfaces = useSurfacesStore();
const flows = useFlowsStore();
const catalog = useCatalogStore();
const layer = computed(() => surfaces.layer(props.id));

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
const grid = computed(() => gridOf(layer.value?.controls ?? []));
const controls = computed(() => grid.value.cells.map((c) => c.control.id));
/** Whether a control shows an image, which is all M5's bindings can set. */
const hasImage = (control: string) =>
  layer.value?.controls.find((c) => c.id === control)?.outputs?.includes("image") ?? false;
const failing = (control: string) => {
  const id = byControl.value.get(control)?.id;
  return id !== undefined && runtime.value.some((p) => p.binding === id);
};
/** Flows with a gesture trigger on each control, for the grid's corner mark. */
const triggered = computed(() => {
  const out = new Set<string>();
  for (const f of flows.flows) {
    for (const t of f.triggers) if (t.kind === "gesture" && t.control.layer === props.id) out.add(t.control.control);
  }
  return out;
});

function select(control: string) {
  navigate({ tab: "surfaces", layer: props.id, control });
}

// Arrow keys move the selection to the nearest control that way.
const steps: Record<string, [number, number]> = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
function move(ev: KeyboardEvent) {
  const step = steps[ev.key];
  if (step === undefined || props.control === undefined) return;
  const next = neighbour(grid.value.cells, props.control, ...step);
  if (next === undefined) return;
  ev.preventDefault();
  select(next);
  const target = [...(ev.currentTarget as HTMLElement).querySelectorAll<HTMLElement>("[data-control]")].find(
    (el) => el.dataset.control === next,
  );
  target?.focus();
}

const runtimeFor = computed(() => {
  const id = props.control === undefined ? undefined : byControl.value.get(props.control)?.id;
  return runtime.value.filter((p) => p.binding === id);
});

// ---- Where it's shown ----

const live = useLiveState("surface");
const shownOn = computed(() =>
  surfaces.surfaces.filter((s) => {
    const leaf = live.leaves.value.find((l) => l.path === `surface.${s.slug}.current.main`);
    return layer.value !== undefined && leaf?.value === layer.value.slug;
  }),
);

// ---- Settings ----

const draft = ref<Layer | null>(null);
const saveError = ref<string | null>(null);
const savedNote = ref<string | null>(null);

watch(
  [() => props.id, () => layer.value !== undefined],
  () => {
    draft.value = layer.value === undefined ? null : { ...layer.value };
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
    const saved = await surfaces.saveLayer({ ...draft.value, name: draft.value.name.trim() });
    draft.value = { ...saved.layer };
    savedNote.value = saved.warnings.length > 0 ? saved.warnings.map((w) => w.message).join(" ") : "Saved.";
  } catch (e) {
    const found = e instanceof ApiError ? e.problems : null;
    saveError.value = found !== null ? found.errors.map((p) => p.message).join(" ") : e instanceof Error ? e.message : String(e);
  }
}

async function remove() {
  const p = layer.value;
  if (p === undefined) return;
  const n = bindings.value.length;
  const also = n === 0 ? "" : ` Its ${n} ${n === 1 ? "binding is" : "bindings are"} deleted too.`;
  if (!confirm(`Delete the layer "${p.name}"?${also}`)) return;
  try {
    await surfaces.removeLayer(p.id);
    navigate({ tab: "surfaces" });
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <p>
    <a :href="hrefOf({ tab: 'surfaces' })">← Surfaces and layers</a>
  </p>
  <p
    v-if="layer === undefined || draft === null"
    class="muted panel"
  >
    {{ surfaces.loaded ? "No such layer." : "Loading…" }}
  </p>
  <template v-else>
    <section class="panel">
      <div class="row title">
        <h2>{{ layer.name }}</h2>
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
          :style="{
            gridTemplateColumns: `repeat(${grid.cols}, var(--key))`,
            gridTemplateRows: `repeat(${grid.rows}, var(--key))`,
          }"
          role="listbox"
          aria-label="Controls"
          @keydown="move"
        >
          <button
            v-for="cell in grid.cells"
            :key="cell.control.id"
            type="button"
            class="key"
            role="option"
            :style="{ gridColumn: `${cell.x + 1} / span ${cell.w}`, gridRow: `${cell.y + 1} / span ${cell.h}` }"
            :aria-selected="cell.control.id === control"
            :aria-label="`Control ${cell.control.id}${byControl.has(cell.control.id) ? ', bound' : ''}`"
            :class="{ selected: cell.control.id === control, failing: failing(cell.control.id), plain: !hasImage(cell.control.id) }"
            :data-control="cell.control.id"
            @click="select(cell.control.id)"
          >
            <img
              v-if="results.get(cell.control.id)?.preview"
              :src="`data:image/png;base64,${results.get(cell.control.id)!.preview!.image}`"
              alt=""
            >
            <span
              v-else
              class="key-id"
            >{{ cell.control.id }}</span>
            <span
              v-if="triggered.has(cell.control.id)"
              class="mark"
              title="A flow triggers on this key"
            />
          </button>
        </div>
        <div class="selected-key">
          <p
            v-if="control !== undefined && controls.includes(control) && !hasImage(control)"
            class="muted"
          >
            {{ control }} has no image output on this layer, so there's nothing to bind on it yet. Flows can still
            trigger on it.
          </p>
          <BindingEditor
            v-else-if="control !== undefined && controls.includes(control)"
            :key="`${layer.id}/${control}`"
            :layer="layer"
            :control="control"
            :stored="byControl.get(control)"
            :runtime="runtimeFor"
            @saved="load"
          />
          <p
            v-else
            class="muted"
          >
            Pick a control to bind what it shows: a label, a text colour and a background, each from live state.
          </p>
        </div>
      </div>
    </section>

    <form
      class="panel"
      novalidate
      @submit.prevent="saveSettings"
    >
      <h3>Layer</h3>
      <div class="row">
        <div class="field">
          <label for="layer-name">Name</label>
          <input
            id="layer-name"
            v-model="draft.name"
            autocomplete="off"
          >
          <small class="hint">
            {{ layer.controls.length }} {{ layer.controls.length === 1 ? "control" : "controls" }}, copied from a
            surface when it was made. A surface shows it if it has each of them.
          </small>
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
  width: 100%;
  height: 100%;
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

.key.plain {
  background: #16171a;
}

button.danger {
  border-color: var(--bad);
  color: var(--bad);
}
</style>
