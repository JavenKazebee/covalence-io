<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ApiError } from "../api";
import {
  concurrencies,
  freshStepId,
  fromDraft,
  newFlowDraft,
  newStepDraft,
  paramProblems,
  paramsSchemaFrom,
  pathText,
  problemsAt,
  toDraft,
  type Concurrency,
  type FlowDraft,
  type ShownProblem,
  type StepDraft,
} from "../flows/model";
import type { FlowStatus } from "../flows/status";
import { hrefOf, navigate } from "../nav";
import { useCatalogStore } from "../stores/catalog";
import { useFlowsStore } from "../stores/flows";
import FlowRuns from "./FlowRuns.vue";
import StepEditor from "./StepEditor.vue";

/**
 * Creates or edits one flow (flow spec §8): name, triggers, concurrency and
 * a flat list of action and wait steps. The server validates on save and
 * its problems show next to the field they name.
 */
const props = defineProps<{ id: string; status: FlowStatus | undefined }>();

const catalog = useCatalogStore();
const flows = useFlowsStore();

const schemaOf = paramsSchemaFrom((connId) => {
  const conn = catalog.connections.find((c) => c.id === connId);
  return conn === undefined ? null : catalog.manifest(conn.plugin);
});

const isNew = computed(() => props.id === "new");
const stored = computed(() => (isNew.value ? undefined : flows.get(props.id)));
const draft = ref<FlowDraft | null>(null);
const problems = ref<ShownProblem[]>([]);
const localErrors = ref(new Map<number, Record<string, string>>());
const saving = ref(false);
const saveError = ref<string | null>(null);
const savedNote = ref<string | null>(null);
const currentStep = ref<string | null>(null);

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
    if (isNew.value) draft.value = newFlowDraft();
    else draft.value = stored.value === undefined ? null : toDraft(stored.value, schemaOf);
  },
  { immediate: true },
);

const at = (...prefix: (string | number)[]) => problemsAt(problems.value, prefix);
const stepProblems = (i: number) => problemsAt(problems.value, ["body", i], [["body", i, "params"]]);
const stepParamErrors = (step: StepDraft, i: number) => ({ ...paramProblems(problems.value, i), ...localErrors.value.get(step.key) });
/** Problems no field below shows: anything outside name, slug, triggers and body. */
const otherProblems = computed(() =>
  problems.value.filter((p) => !["name", "slug", "triggers", "body", "concurrency"].includes(String(p.path[0]))),
);

const concurrencyHelp: Record<Concurrency, string> = {
  parallel: "Start another run alongside the active one",
  queue: "Run it after the active run finishes",
  restart: "Cancel the active run, then start again",
  ignore: "Drop the new trigger while a run is active",
};

// ---- Triggers ----

const hasApi = computed(() => draft.value?.triggers.some((t) => t.kind === "api") ?? false);

function eventsOf(connId: string) {
  const conn = catalog.connections.find((c) => c.id === connId);
  return conn === undefined ? [] : (catalog.manifest(conn.plugin)?.events ?? []);
}

function addTrigger(kind: "api" | "event") {
  if (draft.value === null) return;
  if (kind === "api") {
    draft.value.triggers.push({ kind: "api" });
    return;
  }
  const conn = catalog.connections[0]?.id ?? "";
  draft.value.triggers.push({ kind: "event", source: { conn }, event: eventsOf(conn)[0]?.id ?? "" });
}

function setEventConn(index: number, conn: string) {
  const trigger = draft.value?.triggers[index];
  if (trigger?.kind !== "event") return;
  trigger.source = { conn };
  const events = eventsOf(conn);
  if (!events.some((e) => e.id === trigger.event)) trigger.event = events[0]?.id ?? "";
}

// ---- Steps ----

function addStep(kind: StepDraft["kind"]) {
  const d = draft.value;
  if (d === null) return;
  const ids = d.body.map((s) => s.id);
  if (kind === "wait") {
    d.body.push(newStepDraft("wait", freshStepId("wait", ids)));
    return;
  }
  // Start from the last action's target, the usual next step.
  const last = [...d.body].reverse().find((s) => s.kind === "action");
  const conn = last?.conn ?? catalog.connections[0]?.id ?? "";
  const plugin = catalog.connections.find((c) => c.id === conn)?.plugin;
  const action = last?.action ?? (plugin === undefined ? undefined : catalog.manifest(plugin)?.actions[0]?.id) ?? "";
  const step = newStepDraft("action", freshStepId(action || "step", ids));
  step.conn = conn;
  step.action = action;
  d.body.push(step);
}

function moveStep(index: number, by: -1 | 1) {
  const body = draft.value?.body;
  if (body === undefined) return;
  const [step] = body.splice(index, 1);
  if (step !== undefined) body.splice(index + by, 0, step);
  // The server's paths name steps by index, so they no longer line up.
  problems.value = problems.value.filter((p) => p.path[0] !== "body");
}

function removeStep(index: number) {
  draft.value?.body.splice(index, 1);
  problems.value = problems.value.filter((p) => p.path[0] !== "body");
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
          <template v-else>
            <strong>Event</strong>
            <select
              :value="trigger.source.conn"
              :aria-label="`Trigger ${i + 1} connection`"
              @change="setEventConn(i, ($event.target as HTMLSelectElement).value)"
            >
              <option
                v-if="!catalog.connections.some((c) => c.id === trigger.source.conn)"
                :value="trigger.source.conn"
              >
                {{ trigger.source.conn || "Pick a connection" }}
              </option>
              <option
                v-for="c in catalog.connections"
                :key="c.id"
                :value="c.id"
              >
                {{ c.alias }}
              </option>
            </select>
            <select
              v-model="trigger.event"
              :aria-label="`Trigger ${i + 1} event`"
            >
              <option
                v-if="!eventsOf(trigger.source.conn).some((e) => e.id === trigger.event)"
                :value="trigger.event"
              >
                {{ trigger.event || "No events" }}
              </option>
              <option
                v-for="e in eventsOf(trigger.source.conn)"
                :key="e.id"
                :value="e.id"
              >
                {{ e.name === e.id ? e.id : `${e.name} (${e.id})` }}
              </option>
            </select>
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
            v-if="at('triggers', i).length > 0"
            class="problems"
          >
            <li
              v-for="(p, j) in at('triggers', i)"
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
          :disabled="catalog.connections.length === 0"
          @click="addTrigger('event')"
        >
          Add event trigger
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
      <h3>Steps</h3>
      <p class="muted">
        Run in order, top to bottom.
      </p>
      <ol class="steps">
        <StepEditor
          v-for="(step, i) in draft.body"
          :key="step.key"
          v-model="draft.body[i]!"
          :index="i"
          :count="draft.body.length"
          :other-ids="draft.body.filter((s) => s !== step).map((s) => s.id)"
          :problems="stepProblems(i)"
          :param-errors="stepParamErrors(step, i)"
          :current="currentStep !== null && currentStep === step.id"
          @move="(by) => moveStep(i, by)"
          @remove="removeStep(i)"
        />
      </ol>
      <p
        v-if="draft.body.length === 0"
        class="muted"
      >
        No steps yet.
      </p>
      <div class="row">
        <button
          type="button"
          class="secondary"
          :disabled="catalog.connections.length === 0"
          @click="addStep('action')"
        >
          Add action
        </button>
        <button
          type="button"
          class="secondary"
          @click="addStep('wait')"
        >
          Add wait
        </button>
      </div>
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
    @position="(id) => (currentStep = id)"
  />
</template>

<style scoped>
.triggers,
.steps {
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
