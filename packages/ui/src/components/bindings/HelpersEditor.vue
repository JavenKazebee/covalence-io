<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { api } from "../../api";
import { helperPath, type Helper } from "../../bindings/model";
import { useLiveState } from "../../live/useLiveState";
import type { Json } from "../../sources/model";
import type { Layer } from "../../surfaces/model";
import { omit, reducers, type CellProblem, type Sink } from "../../variables/model";
import SinksEditor from "../SinksEditor.vue";

/**
 * A control's helpers (bindings spec §5): reducers on its input, each with a
 * live readout of its cell, its options and its sinks.
 */
const props = defineProps<{ layer: Layer; control: string; binding: string }>();
const helpers = defineModel<Record<string, Helper> | undefined>({ required: true });

const entries = computed(() => Object.entries(helpers.value ?? {}));
function set(next: Record<string, Helper>) {
  helpers.value = Object.keys(next).length === 0 ? undefined : next;
}
function replace(key: string, helper: Helper, newKey = key) {
  const next: Record<string, Helper> = {};
  for (const [k, h] of entries.value) {
    if (k === key) next[newKey] = helper;
    else next[k] = h;
  }
  set(next);
}
function add() {
  const taken = new Set(Object.keys(helpers.value ?? {}));
  let key = "toggle";
  for (let n = 2; taken.has(key); n++) key = `toggle${n}`;
  set({ ...(helpers.value ?? {}), [key]: { use: "toggle" } });
}
function remove(key: string) {
  set(Object.fromEntries(entries.value.filter(([k]) => k !== key)));
}
function rename(key: string, helper: Helper, next: string) {
  const name = next.trim();
  if (name === "" || name === key || (helpers.value ?? {})[name] !== undefined) return;
  replace(key, helper, name);
}

// Options: everything but the fields the editor shows on their own.
const own = ["use", "as", "scope", "to", "journal"] as const;
const optionsText = ref(new Map<string, { text: string; bad: boolean }>());
const textOf = (key: string, h: Helper) => {
  const kept = optionsText.value.get(key);
  if (kept?.bad) return kept.text;
  const o = omit(h as Record<string, unknown>, own);
  return Object.keys(o).length === 0 ? "" : JSON.stringify(o, null, 2);
};
const badOf = (key: string) => optionsText.value.get(key)?.bad ?? false;
function editOptions(key: string, h: Helper, text: string) {
  let parsed: Record<string, Json> = {};
  try {
    const v = text.trim() === "" ? {} : (JSON.parse(text) as Json);
    if (typeof v !== "object" || v === null || Array.isArray(v)) throw new Error("not an object");
    parsed = v;
  } catch {
    optionsText.value = new Map(optionsText.value).set(key, { text, bad: true });
    return;
  }
  optionsText.value = new Map(optionsText.value).set(key, { text, bad: false });
  const kept = Object.fromEntries(own.filter((k) => h[k] !== undefined).map((k) => [k, h[k]]));
  replace(key, { ...parsed, ...kept } as Helper);
}
/** Whether an options field doesn't parse, so the binding shouldn't be saved. */
const anyBad = computed(() => [...optionsText.value.values()].some((o) => o.bad));
defineExpose({ anyBad });

// ---- Live values ----

const layerLive = useLiveState(() => `layer.${props.layer.slug}.controls.${props.control}`);
const surfaceLive = useLiveState("surface");
function valuesOf(key: string, h: Helper): { where: string; value: string }[] {
  if (h.scope !== "surface") {
    const path = helperPath(props.layer.slug, props.control, key, h);
    const leaf = layerLive.leaves.value.find((l) => l.path === path);
    return [{ where: path, value: leaf === undefined ? "no value yet" : JSON.stringify(leaf.value) }];
  }
  const suffix = `.controls.${props.control}.helpers.${h.as ?? key}`;
  return surfaceLive.leaves.value
    .filter((l) => l.path.endsWith(suffix))
    .map((l) => ({ where: l.path, value: JSON.stringify(l.value) }));
}

const problems = ref<CellProblem[]>([]);
async function loadProblems() {
  try {
    problems.value = await api.cellProblems();
  } catch {
    // Kept as they were; the next try may work.
  }
}
void loadProblems();
const timer = setInterval(loadProblems, 5000);
onBeforeUnmount(() => clearInterval(timer));
const problemOf = (key: string) =>
  problems.value.find((p) => p.cell === `helper:${props.binding}/${key}` || p.cell.startsWith(`helper:${props.binding}/${key}@`));

const sinksOf = (h: Helper) => h.to ?? [];
function setSinks(key: string, h: Helper, to: Sink[]) {
  replace(key, to.length === 0 ? (omit(h, ["to"]) as Helper) : { ...h, to });
}
</script>

<template>
  <fieldset class="helpers">
    <legend>Helpers</legend>
    <p
      v-if="entries.length === 0"
      class="muted"
    >
      None. A helper turns this control's input into a value: a latch, a position, a selection. Its value is state
      bindings and flows can read, and it can send it on.
    </p>
    <div
      v-for="[key, h] in entries"
      :key="key"
      class="helper"
      :data-helper="key"
    >
      <div class="row">
        <div class="field">
          <label :for="`helper-${key}-name`">Name</label>
          <input
            :id="`helper-${key}-name`"
            :value="key"
            autocomplete="off"
            @change="rename(key, h, ($event.target as HTMLInputElement).value)"
          >
        </div>
        <div class="field">
          <label :for="`helper-${key}-use`">Reducer</label>
          <select
            :id="`helper-${key}-use`"
            :value="h.use.split('@')[0]"
            @change="replace(key, { ...h, use: ($event.target as HTMLSelectElement).value })"
          >
            <option
              v-for="r in reducers"
              :key="r"
              :value="r"
            >
              {{ r }}
            </option>
          </select>
        </div>
        <div class="field">
          <label :for="`helper-${key}-scope`">One value</label>
          <select
            :id="`helper-${key}-scope`"
            :value="h.scope ?? 'layer'"
            @change="
              replace(
                key,
                ($event.target as HTMLSelectElement).value === 'surface'
                  ? { ...h, scope: 'surface' }
                  : (omit(h, ['scope']) as Helper),
              )
            "
          >
            <option value="layer">
              for the layer
            </option>
            <option value="surface">
              per surface
            </option>
          </select>
        </div>
        <button
          type="button"
          class="secondary"
          @click="remove(key)"
        >
          Remove
        </button>
      </div>
      <p
        class="readout"
        data-testid="helper-value"
      >
        <template
          v-for="v in valuesOf(key, h)"
          :key="v.where"
        >
          <code>{{ v.where }}</code> = <code>{{ v.value }}</code><br>
        </template>
        <span
          v-if="valuesOf(key, h).length === 0"
          class="muted"
        >No surface has shown this layer yet.</span>
        <span
          v-if="problemOf(key)"
          class="error"
        >{{ problemOf(key)!.code }}: {{ problemOf(key)!.message }}</span>
      </p>
      <div class="field">
        <label :for="`helper-${key}-options`">Options (JSON): <code>max</code>, <code>items</code>, <code>follow</code>…</label>
        <textarea
          :id="`helper-${key}-options`"
          rows="3"
          :value="textOf(key, h)"
          :aria-invalid="badOf(key)"
          placeholder="none: it steps on this control's input"
          @input="editOptions(key, h, ($event.target as HTMLTextAreaElement).value)"
        />
        <small
          v-if="badOf(key)"
          class="error"
        >Not a JSON object.</small>
      </div>
      <h4>Sends its value to</h4>
      <SinksEditor
        :model-value="sinksOf(h)"
        :id-prefix="`helper-${key}-sink`"
        @update:model-value="setSinks(key, h, $event)"
      />
    </div>
    <button
      type="button"
      class="secondary"
      @click="add"
    >
      Add a helper
    </button>
  </fieldset>
</template>

<style scoped>
.helpers {
  border: 1px solid var(--border, #d8d9de);
  border-radius: 8px;
  padding: 0.75rem;
  margin: 0.75rem 0;
}

.helper {
  border-bottom: 1px solid var(--border, #d8d9de);
  padding-bottom: 0.75rem;
  margin-bottom: 0.75rem;
}

.helper textarea {
  width: 100%;
  font-family: var(--mono, monospace);
}

.readout {
  font-size: 0.9em;
}

h4 {
  margin: 0.5rem 0 0.25rem;
}
</style>
