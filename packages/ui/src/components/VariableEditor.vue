<script setup lang="ts">
import { computed, shallowRef, watch } from "vue";
import { api, ApiError } from "../api";
import type { ShownProblem } from "../flows/model";
import { useLiveState } from "../live/useLiveState";
import { hrefOf, navigate } from "../nav";
import type { Json, Source } from "../sources/model";
import {
  blankVariable,
  kindNames,
  kindOf,
  omit,
  reducerOf,
  reducerOptions,
  reducers,
  reducerSource,
  schemaTypes,
  withKind,
  type CellProblem,
  type Kind,
  type ReducerName,
  type SchemaType,
  type Sink,
  type Variable,
} from "../variables/model";
import SinksEditor from "./SinksEditor.vue";
import SourceInput from "./SourceInput.vue";

/**
 * One variable (dataflow spec §6): writable, computed from a source, or
 * accumulated by a reducer, with sinks that keep other things in step with
 * a computed one (§7).
 */
const props = defineProps<{ id: string; variables: readonly Variable[]; problem?: CellProblem }>();
const emit = defineEmits<{ saved: [] }>();

const stored = computed(() => props.variables.find((v) => v.id === props.id));
const isNew = computed(() => props.id === "new");

// Shallow, as its sources nest deeply: changed by replacing it (`patch`).
const draft = shallowRef<Variable | null>(null);
const problems = shallowRef<ShownProblem[]>([]);
const note = shallowRef<{ tone: "bad" | "muted"; text: string } | null>(null);

watch(
  [() => props.id, () => stored.value !== undefined],
  () => {
    draft.value = isNew.value ? blankVariable() : stored.value === undefined ? null : { ...stored.value };
    problems.value = [];
    note.value = null;
  },
  { immediate: true },
);

function patch(fields: Partial<Variable>) {
  if (draft.value !== null) draft.value = { ...draft.value, ...fields };
}

const kind = computed<Kind>({
  get: () => (draft.value === null ? "writable" : kindOf(draft.value)),
  set: (k) => {
    if (draft.value !== null) draft.value = withKind(draft.value, k);
  },
});
const kinds = Object.keys(kindNames) as Kind[];

const type = computed<SchemaType>({
  get: () => draft.value?.schema?.type ?? "any",
  set: (t) => {
    const rest = omit(draft.value?.schema ?? {}, ["type"]);
    patch({ schema: t === "any" ? (Object.keys(rest).length > 0 ? rest : undefined) : { ...rest, type: t } });
  },
});

/** A JSON text field: what was typed, and whether it parses. */
function jsonText(get: () => Json | undefined, set: (v: Json | undefined) => void) {
  const text = shallowRef("");
  const bad = shallowRef(false);
  watch(
    () => draft.value,
    () => {
      if (!bad.value) {
        const v = get();
        text.value = v === undefined ? "" : JSON.stringify(v, null, 2);
      }
    },
    { immediate: true },
  );
  function input(next: string) {
    text.value = next;
    if (next.trim() === "") {
      bad.value = false;
      set(undefined);
      return;
    }
    try {
      const parsed = JSON.parse(next) as Json;
      bad.value = false;
      set(parsed);
    } catch {
      bad.value = true;
    }
  }
  return { text, bad, input };
}

const initial = jsonText(
  () => draft.value?.initial,
  (v) => patch({ initial: v }),
);

const computedValue = computed<Source>({
  get: () => draft.value?.value ?? { expr: "" },
  set: (value) => patch({ value }),
});

const reducer = computed<ReducerName>({
  get: () => reducerOf(draft.value?.value) ?? "toggle",
  set: (name) => patch({ value: reducerSource(name, reducerOptions(draft.value?.value)) }),
});
const options = jsonText(
  () => reducerOptions(draft.value?.value),
  (v) =>
    patch({
      value: reducerSource(reducer.value, typeof v === "object" && v !== null && !Array.isArray(v) ? v : {}),
    }),
);

const sinks = computed<Sink[]>({
  get: () => draft.value?.to ?? [],
  set: (to) => patch({ to: to.length === 0 ? undefined : to }),
});

// ---- Live value, saving ----

const live = useLiveState(() => (draft.value?.slug ? `vars.${draft.value.slug}` : ""));
const liveValue = computed(() => (live.leaf.value === undefined ? "—" : JSON.stringify(live.leaf.value.value)));

const canSave = computed(() => !initial.bad.value && !options.bad.value);

async function save() {
  const d = draft.value;
  if (d === null) return;
  note.value = null;
  problems.value = [];
  try {
    const body = { ...d, name: d.name.trim() };
    const saved = isNew.value ? await api.createVariable(body) : await api.updateVariable(body);
    problems.value = saved.warnings.map((w) => ({ ...w, warning: true }));
    note.value = { tone: "muted", text: "Saved." };
    emit("saved");
    if (isNew.value) navigate({ tab: "variables", variable: saved.variable.id });
    else draft.value = { ...saved.variable };
  } catch (e) {
    const found = e instanceof ApiError ? e.problems : null;
    if (found !== null) {
      problems.value = [
        ...found.errors.map((p) => ({ ...p, warning: false })),
        ...found.warnings.map((p) => ({ ...p, warning: true })),
      ];
      note.value = { tone: "bad", text: "Not saved: see the problems below." };
    } else {
      note.value = { tone: "bad", text: e instanceof Error ? e.message : String(e) };
    }
  }
}

async function remove() {
  const d = draft.value;
  if (d === null || isNew.value) return;
  if (!confirm(`Delete the variable "${d.name}"? Its value at vars.${d.slug} goes too.`)) return;
  try {
    await api.deleteVariable(d.id);
    emit("saved");
    navigate({ tab: "variables" });
  } catch (e) {
    note.value = { tone: "bad", text: e instanceof Error ? e.message : String(e) };
  }
}

const valueProblems = computed(() => problems.value.filter((p) => p.path[0] === "value"));
const otherProblems = computed(() => problems.value.filter((p) => p.path[0] !== "value"));
</script>

<template>
  <p>
    <a :href="hrefOf({ tab: 'variables' })">← Variables</a>
  </p>
  <p
    v-if="draft === null"
    class="muted panel"
  >
    No such variable.
  </p>
  <form
    v-else
    class="panel"
    novalidate
    @submit.prevent="save"
  >
    <div class="row title">
      <h2>{{ isNew ? "New variable" : draft.name }}</h2>
      <span
        v-if="!isNew"
        class="muted"
      >
        <code>vars.{{ draft.slug }}</code> = <code data-testid="live-value">{{ liveValue }}</code>
        <span
          v-if="problem"
          class="error"
        > {{ problem.code }}: {{ problem.message }}</span>
      </span>
    </div>
    <div class="row">
      <div class="field">
        <label for="var-name">Name</label>
        <input
          id="var-name"
          :value="draft.name"
          autocomplete="off"
          @input="patch({ name: ($event.target as HTMLInputElement).value })"
        >
      </div>
      <div class="field">
        <label for="var-slug">Path name</label>
        <input
          id="var-slug"
          :value="draft.slug"
          autocomplete="off"
          placeholder="from the name"
          @input="patch({ slug: ($event.target as HTMLInputElement).value })"
        >
        <small class="hint">Read as <code>vars.{{ draft.slug || "…" }}</code>. Renaming keeps its value.</small>
      </div>
      <div class="field">
        <label for="var-type">Type</label>
        <select
          id="var-type"
          v-model="type"
        >
          <option
            v-for="t in schemaTypes"
            :key="t"
            :value="t"
          >
            {{ t }}
          </option>
        </select>
      </div>
      <div class="field">
        <label>
          <input
            type="checkbox"
            :checked="draft.persist ?? false"
            @change="patch({ persist: ($event.target as HTMLInputElement).checked || undefined })"
          >
          Keep across a restart
        </label>
      </div>
    </div>

    <fieldset class="kind">
      <legend>Its value</legend>
      <label
        v-for="k in kinds"
        :key="k"
      >
        <input
          v-model="kind"
          type="radio"
          name="kind"
          :value="k"
        >
        {{ kindNames[k] }}
      </label>
    </fieldset>

    <div
      v-if="kind === 'writable'"
      class="field"
    >
      <label for="var-initial">Initial value (JSON)</label>
      <input
        id="var-initial"
        :value="initial.text.value"
        autocomplete="off"
        placeholder="none"
        :aria-invalid="initial.bad.value"
        @input="initial.input(($event.target as HTMLInputElement).value)"
      >
      <small
        v-if="initial.bad.value"
        class="error"
      >Not JSON: <code>false</code>, <code>0</code>, <code>"text"</code>…</small>
      <small
        v-else
        class="hint"
      >Its value when the show loads, until a flow sets it.</small>
    </div>
    <SourceInput
      v-else-if="kind === 'computed'"
      v-model="computedValue"
      label="Computed from"
      :problems="valueProblems"
      top
    />
    <div
      v-else
      class="row"
    >
      <div class="field">
        <label for="var-reducer">Reducer</label>
        <select
          id="var-reducer"
          v-model="reducer"
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
      <div class="field grow">
        <label for="var-options">Options (JSON): <code>on</code>, <code>input</code>, <code>follow</code>…</label>
        <textarea
          id="var-options"
          rows="5"
          :value="options.text.value"
          :aria-invalid="options.bad.value"
          @input="options.input(($event.target as HTMLTextAreaElement).value)"
        />
        <small
          v-if="options.bad.value"
          class="error"
        >Not a JSON object.</small>
        <small
          v-for="(p, i) in valueProblems"
          :key="i"
          :class="p.warning ? 'hint' : 'error'"
        >{{ p.message }}</small>
      </div>
    </div>

    <section
      v-if="kind !== 'writable'"
      class="sinks"
    >
      <h3>Sends its value to</h3>
      <SinksEditor
        v-model="sinks"
        id-prefix="var-sink"
      />
    </section>

    <ul
      v-if="otherProblems.length > 0"
      class="problems"
    >
      <li
        v-for="(p, i) in otherProblems"
        :key="i"
        :class="p.warning ? 'muted' : 'error'"
      >
        <code>{{ p.path.join(".") }}</code>: {{ p.message }}
      </li>
    </ul>
    <div class="row">
      <button
        type="submit"
        :disabled="!canSave"
      >
        Save
      </button>
      <button
        v-if="!isNew"
        type="button"
        class="secondary danger"
        @click="remove"
      >
        Delete
      </button>
      <span
        v-if="note"
        :class="note.tone === 'bad' ? 'error' : 'muted'"
        role="status"
      >{{ note.text }}</span>
    </div>
  </form>
</template>

<style scoped>
.title {
  justify-content: space-between;
  margin-bottom: 0.75rem;
}

.title h2 {
  margin: 0;
}

.kind {
  display: flex;
  flex-wrap: wrap;
  gap: 1rem;
  border: none;
  padding: 0;
  margin: 0.75rem 0;
}

.grow {
  flex: 1;
}

.grow textarea {
  width: 100%;
  font-family: var(--mono, monospace);
}

.sinks {
  margin: 1rem 0;
}

button.danger {
  border-color: var(--bad);
  color: var(--bad);
}
</style>
