<script setup lang="ts">
import { computed, ref, watch, type Ref } from "vue";
import { ApiError } from "../api";
import {
  allSteps,
  concurrencies,
  edges,
  fromDraft,
  newFlowDraft,
  paramsSchemaFrom,
  pathText,
  problemsAt,
  toDraft,
  type Concurrency,
  type Edge,
  type FlowDraft,
  type PathSegment,
  type ShownProblem,
  type StepContext,
  type Trigger,
  eventNames,
} from "../flows/model";
import type { FlowStatus } from "../flows/status";
import { hrefOf, navigate } from "../nav";
import { useCatalogStore } from "../stores/catalog";
import { useFlowsStore } from "../stores/flows";
import { useSurfacesStore } from "../stores/surfaces";
import { newSource } from "../sources/model";
import EventSourceInput from "./EventSourceInput.vue";
import ExprInput from "./ExprInput.vue";
import SourceInput from "./SourceInput.vue";
import FlowIoEditor from "./FlowIoEditor.vue";
import FlowRuns from "./FlowRuns.vue";
import StepList from "./StepList.vue";

/**
 * Creates or edits one flow (flow spec §8): name, triggers, conditions,
 * concurrency and its steps, which `branch` steps nest. The server validates
 * on save and its problems show next to the field they name.
 */
const props = defineProps<{ id: string; status: FlowStatus | undefined }>();

const catalog = useCatalogStore();
const flows = useFlowsStore();
const surfaces = useSurfacesStore();

const schemaOf = paramsSchemaFrom(
  (connId) => {
    const conn = catalog.connections.find((c) => c.id === connId);
    return conn === undefined ? null : catalog.manifest(conn.plugin);
  },
  (plugin) => catalog.manifest(plugin),
);

const isNew = computed(() => props.id === "new");
const stored = computed(() => (isNew.value ? undefined : flows.get(props.id)));
// Deeply reactive, but typed without unwrapping: sources nest too deeply for `UnwrapRef`.
const draft = ref(null) as Ref<FlowDraft | null>;
const problems = ref<ShownProblem[]>([]);
const localErrors = ref(new Map<number, Record<string, string>>());
const saving = ref(false);
const saveError = ref<string | null>(null);
const savedNote = ref<string | null>(null);
/** The active run's position, for highlighting its steps. */
const position = ref<string[]>([]);

// Load once per flow: later refreshes of the list mustn't clobber edits.
watch(
  () => [props.id, flows.loaded, stored.value !== undefined] as const,
  ([id], previous) => {
    if (previous !== undefined && previous[0] === id && draft.value !== null) return;
    // A flow just created keeps its save's warnings and note.
    if (previous?.[0] !== "new") {
      problems.value = [];
      savedNote.value = null;
    }
    localErrors.value = new Map();
    saveError.value = null;
    if (isNew.value) {
      // A key's "When pressed" leaves a draft to start from.
      draft.value = flows.prefill ?? newFlowDraft();
      flows.prefill = null;
    }
    else draft.value = stored.value === undefined ? null : toDraft(stored.value, schemaOf);
  },
  { immediate: true },
);

const at = (...prefix: (string | number)[]) => problemsAt(problems.value, prefix);
/** Problems no field below shows: anything outside name, slug, triggers, conditions and body. */
const otherProblems = computed(() =>
  problems.value.filter(
    (p) => !["name", "slug", "triggers", "conditions", "body", "concurrency"].includes(String(p.path[0])),
  ),
);

const ctx = computed<StepContext>(() => {
  const body = draft.value?.body ?? [];
  return { body, all: new Set(allSteps(body).map((s) => s.id)), position: position.value, outputs: draft.value?.outputs ?? {} };
});
const noSteps: ReadonlySet<string> = new Set();

/** After steps move or go, the server's paths below `path` no longer line up. */
function reshaped(path: PathSegment[]) {
  problems.value = problems.value.filter((p) => !path.every((seg, i) => p.path[i] === seg));
}

const concurrencyHelp: Record<Concurrency, string> = {
  parallel: "Start another run alongside the active one",
  queue: "Run it after the active run finishes",
  restart: "Cancel the active run, then start again",
  ignore: "Drop the new trigger while a run is active",
};

// ---- Triggers ----

const hasApi = computed(() => draft.value?.triggers.some((t) => t.kind === "api") ?? false);
const hasCall = computed(() => draft.value?.triggers.some((t) => t.kind === "call") ?? false);

function addTrigger(kind: "api" | "call" | "event" | "state") {
  if (draft.value === null) return;
  if (kind === "api" || kind === "call") {
    draft.value.triggers.push({ kind });
    return;
  }
  if (kind === "state") {
    draft.value.triggers.push({ kind: "state", when: "", edge: "rising" });
    return;
  }
  // A key's press is the usual start; a connection's events otherwise.
  const layer = surfaces.layers[0];
  if (layer !== undefined) {
    draft.value.triggers.push({ kind: "event", from: { layer: layer.id, controls: layer.controls[0]?.id ?? "k0" }, event: "press" });
    return;
  }
  const conn = catalog.connections.find((c) => !c.builtIn);
  const event = conn === undefined ? "" : (catalog.manifest(conn.plugin)?.events?.[0]?.id ?? "");
  draft.value.triggers.push({ kind: "event", from: { target: conn?.id ?? "" }, event });
}

const edgeHelp: Record<Edge, string> = {
  rising: "When it becomes true",
  falling: "When it becomes false",
  change: "Whenever its value changes",
};

/** A sample event trigger for previewing a filter: the payload isn't known until it fires. */
const sampleEvent = (event: string | string[]) => ({ path: "", event: eventNames(event)[0] ?? "", payload: {} });

/** A state trigger's `when`: an expression's text, or a structured source. */
type StateTrigger = Extract<Trigger, { kind: "state" }>;
const whenIsText = (t: StateTrigger) => t.when === undefined || typeof t.when === "string";

function setStateMode(t: StateTrigger, pattern: boolean) {
  if (pattern) t.match = "surface.*.controls.*.pressed";
  else {
    delete t.match;
    if (t.when === undefined) t.when = "";
  }
}

function setWhenMode(t: StateTrigger, structured: boolean) {
  t.when = structured ? newSource("compare") : "";
}

// ---- Saving ----

async function save() {
  const d = draft.value;
  if (d === null) return;
  saveError.value = null;
  savedNote.value = null;
  const result = fromDraft(d, schemaOf);
  localErrors.value = result.ok ? new Map() : result.errors;
  if (!result.ok) {
    saveError.value = "Fix the highlighted params first.";
    return;
  }
  saving.value = true;
  try {
    const saved = await flows.save(result.flow);
    problems.value = saved.warnings.map((p) => ({ ...p, warning: true }));
    savedNote.value = saved.warnings.length > 0 ? "Saved, with warnings." : "Saved.";
    draft.value = toDraft(saved.flow, schemaOf);
    if (isNew.value) navigate({ tab: "flows", flow: saved.flow.id });
  } catch (e) {
    const found = e instanceof ApiError ? e.problems : null;
    if (found !== null) {
      problems.value = [
        ...found.errors.map((p) => ({ ...p, warning: false })),
        ...found.warnings.map((p) => ({ ...p, warning: true })),
      ];
      saveError.value = `Not saved: ${found.errors.length} ${found.errors.length === 1 ? "problem" : "problems"}.`;
    } else {
      saveError.value = e instanceof Error ? e.message : String(e);
    }
  } finally {
    saving.value = false;
  }
}

async function remove() {
  const d = draft.value;
  if (d === null || d.id === "") return;
  if (!confirm(`Delete the flow "${d.name}"? Its active runs are cancelled.`)) return;
  try {
    await flows.remove(d.id);
    navigate({ tab: "flows" });
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <p>
    <a :href="hrefOf({ tab: 'flows' })">← All flows</a>
  </p>
  <p
    v-if="!isNew && draft === null"
    class="muted panel"
  >
    {{ flows.loaded ? "No such flow." : "Loading…" }}
  </p>
  <form
    v-else-if="draft !== null"
    novalidate
    class="flow-editor"
    @submit.prevent="save"
  >
    <section class="panel">
      <h2>{{ isNew ? "New flow" : draft.name || "Flow" }}</h2>
      <div class="row">
        <div class="field">
          <label for="flow-name">Name<span
            class="required"
            title="Required"
          >*</span></label>
          <input
            id="flow-name"
            v-model="draft.name"
            autocomplete="off"
          >
          <small
            v-for="(p, i) in at('name')"
            :key="i"
            class="error"
          >{{ p.message }}</small>
        </div>
        <div class="field">
          <label for="flow-slug">Slug</label>
          <input
            id="flow-slug"
            v-model="draft.slug"
            placeholder="From the name"
            spellcheck="false"
            autocomplete="off"
          >
          <small class="hint">Its status is at <code>flow.{{ draft.slug || "…" }}</code>.</small>
          <small
            v-for="(p, i) in at('slug')"
            :key="i"
            class="error"
          >{{ p.message }}</small>
        </div>
      </div>
      <div class="row">
        <div class="field">
          <label for="flow-concurrency">When it fires during a run</label>
          <select
            id="flow-concurrency"
            v-model="draft.concurrency"
          >
            <option
              v-for="c in concurrencies"
              :key="c"
              :value="c"
            >
              {{ c }}: {{ concurrencyHelp[c] }}
            </option>
          </select>
        </div>
        <label class="field enabled"><span><input
          v-model="draft.enabled"
          type="checkbox"
        > Enabled</span></label>
      </div>
    </section>

    <section class="panel">
      <h3>Triggers</h3>
      <p class="muted">
        Any one starts the flow.
      </p>
      <ul class="triggers">
        <li
          v-for="(trigger, i) in draft.triggers"
          :key="i"
          class="row"
          :data-trigger="trigger.kind"
        >
          <template v-if="trigger.kind === 'api'">
            <span><strong>API</strong> <span class="muted">· the Run button and <code>POST /flows/{id}/run</code></span></span>
          </template>
          <template v-else-if="trigger.kind === 'call'">
            <span><strong>Call</strong> <span class="muted">· a <code>call</code> step in another flow</span></span>
          </template>
          <template v-else-if="trigger.kind === 'state'">
            <strong>State</strong>
            <select
              :value="trigger.match === undefined ? 'value' : 'pattern'"
              :aria-label="`Trigger ${i + 1} watches`"
              @change="setStateMode(trigger, ($event.target as HTMLSelectElement).value === 'pattern')"
            >
              <option value="value">
                A value
              </option>
              <option value="pattern">
                Each path matching
              </option>
            </select>
            <input
              v-if="trigger.match !== undefined"
              v-model="trigger.match"
              :aria-label="`Trigger ${i + 1} pattern`"
              class="mono"
              placeholder="surface.*.controls.*.pressed"
              spellcheck="false"
            >
            <template v-if="trigger.match === undefined || trigger.when !== undefined">
              <span
                v-if="trigger.match !== undefined"
                class="muted"
              >where</span>
              <ExprInput
                v-if="whenIsText(trigger)"
                :model-value="(trigger.when as string | undefined) ?? ''"
                :label="`Trigger ${i + 1} expression`"
                placeholder="vh.outputs[1].source == 3"
                :problems="at('triggers', i, 'when')"
                :known="noSteps"
                @update:model-value="trigger.when = $event"
              />
              <SourceInput
                v-else
                v-model="trigger.when!"
                :label="`Trigger ${i + 1} condition`"
                expect="boolean"
                :problems="at('triggers', i, 'when')"
              />
              <button
                type="button"
                class="secondary icon"
                :aria-label="`Trigger ${i + 1} ${whenIsText(trigger) ? 'as a condition' : 'as an expression'}`"
                :title="whenIsText(trigger) ? 'Build it from parts' : 'Write it as an expression'"
                @click="setWhenMode(trigger, whenIsText(trigger))"
              >
                {{ whenIsText(trigger) ? "≡" : "{ }" }}
              </button>
            </template>
            <select
              v-model="trigger.edge"
              :aria-label="`Trigger ${i + 1} edge`"
            >
              <option
                v-for="e in edges"
                :key="e"
                :value="e"
              >
                {{ edgeHelp[e] }}
              </option>
            </select>
          </template>
          <template v-else>
            <strong>Event</strong>
            <EventSourceInput
              v-model="draft.triggers[i] as Extract<Trigger, { kind: 'event' }>"
              :index="i"
              :problems="at('triggers', i)"
            />
            <button
              v-if="trigger.filter === undefined"
              type="button"
              class="secondary"
              @click="trigger.filter = ''"
            >
              Add filter
            </button>
            <div
              v-else
              class="row filter"
            >
              <span class="muted">only if</span>
              <ExprInput
                v-model="trigger.filter"
                :label="`Trigger ${i + 1} filter`"
                placeholder="trigger.payload.output == 1"
                :problems="at('triggers', i, 'filter')"
                :known="noSteps"
                :trigger="sampleEvent(trigger.event)"
              />
              <button
                type="button"
                class="secondary icon"
                :aria-label="`Remove trigger ${i + 1} filter`"
                title="Remove filter"
                @click="delete trigger.filter"
              >
                ×
              </button>
            </div>
          </template>
          <button
            type="button"
            class="secondary icon"
            :aria-label="`Remove trigger ${i + 1}`"
            title="Remove"
            @click="draft.triggers.splice(i, 1)"
          >
            ×
          </button>
          <ul
            v-if="at('triggers', i).some((p) => !['when', 'filter'].includes(String(p.path[2])))"
            class="problems"
          >
            <li
              v-for="(p, j) in at('triggers', i).filter((p) => !['when', 'filter'].includes(String(p.path[2])))"
              :key="j"
              :class="p.warning ? 'warn' : 'error'"
            >
              {{ p.message }}
            </li>
          </ul>
        </li>
      </ul>
      <p
        v-if="draft.triggers.length === 0"
        class="muted"
      >
        No triggers: the flow can't start.
      </p>
      <div class="row">
        <button
          type="button"
          class="secondary"
          :disabled="hasApi"
          @click="addTrigger('api')"
        >
          Add API trigger
        </button>
        <button
          type="button"
          class="secondary"
          :disabled="hasCall"
          @click="addTrigger('call')"
        >
          Add call trigger
        </button>
        <button
          type="button"
          class="secondary"
          @click="addTrigger('event')"
        >
          Add event trigger
        </button>
        <button
          type="button"
          class="secondary"
          @click="addTrigger('state')"
        >
          Add state trigger
        </button>
      </div>
      <ul
        v-if="problemsAt(problems, ['triggers']).some((p) => p.path.length === 1)"
        class="problems"
      >
        <li
          v-for="(p, j) in problemsAt(problems, ['triggers']).filter((p) => p.path.length === 1)"
          :key="j"
          :class="p.warning ? 'warn' : 'error'"
        >
          {{ p.message }}
        </li>
      </ul>
    </section>

    <section class="panel">
      <h3>Conditions</h3>
      <p class="muted">
        When a trigger fires, the flow runs only if all of these are true.
      </p>
      <ul class="conditions">
        <li
          v-for="(_, i) in draft.conditions"
          :key="i"
          class="row"
        >
          <ExprInput
            v-model="draft.conditions[i]!"
            :label="`Condition ${i + 1}`"
            placeholder="vars.armed"
            :problems="at('conditions', i)"
            :known="noSteps"
          />
          <button
            type="button"
            class="secondary icon"
            :aria-label="`Remove condition ${i + 1}`"
            title="Remove"
            @click="draft.conditions.splice(i, 1); reshaped(['conditions'])"
          >
            ×
          </button>
        </li>
      </ul>
      <button
        type="button"
        class="secondary"
        @click="draft.conditions.push('')"
      >
        Add condition
      </button>
    </section>

    <section class="panel">
      <h3>Inputs and outputs</h3>
      <FlowIoEditor
        v-model:inputs="draft.inputs"
        v-model:outputs="draft.outputs"
        :problems="problems"
      />
    </section>

    <section class="panel">
      <h3>Steps</h3>
      <p class="muted">
        Run in order, top to bottom.
      </p>
      <StepList
        v-model="draft.body"
        :path="['body']"
        :problems="problems"
        :local-errors="localErrors"
        :ctx="ctx"
        @reshape="reshaped"
      />
      <ul
        v-if="problemsAt(problems, ['body']).some((p) => p.path.length === 1)"
        class="problems"
      >
        <li
          v-for="(p, j) in problemsAt(problems, ['body']).filter((p) => p.path.length === 1)"
          :key="j"
          :class="p.warning ? 'warn' : 'error'"
        >
          {{ p.message }}
        </li>
      </ul>
    </section>

    <section class="panel save-bar">
      <ul
        v-if="otherProblems.length > 0"
        class="problems"
      >
        <li
          v-for="(p, i) in otherProblems"
          :key="i"
          :class="p.warning ? 'warn' : 'error'"
        >
          <code>{{ pathText(p.path) }}</code> {{ p.message }}
        </li>
      </ul>
      <div class="row">
        <button
          type="submit"
          :disabled="saving"
        >
          {{ saving ? "Saving…" : isNew ? "Create flow" : "Save" }}
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
    </section>
  </form>

  <FlowRuns
    v-if="stored !== undefined"
    :flow="stored"
    :status="status"
    @position="(p) => (position = p)"
  />
</template>

<style scoped>
.triggers,
.conditions {
  list-style: none;
  padding: 0;
  margin: 0 0 0.75rem;
  display: grid;
  gap: 0.5rem;
}

.triggers > li {
  padding: 0.4rem 0;
  border-bottom: 1px solid var(--border);
}

/* Under the trigger's own row, after its remove button. */
.triggers .filter {
  flex-basis: 100%;
  order: 1;
  gap: 0.5rem;
}

.triggers .problems {
  order: 2;
}

.triggers .problems {
  flex-basis: 100%;
  margin: 0;
}

.enabled {
  align-self: end;
}

button.icon {
  padding: 0.15rem 0.55rem;
}

button.danger {
  border-color: var(--bad);
  color: var(--bad);
}

.save-bar {
  position: sticky;
  bottom: 0;
  z-index: 1;
}
</style>
