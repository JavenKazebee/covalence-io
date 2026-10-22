<script setup lang="ts">
import { computed, watch } from "vue";
import { fieldsOf, initialValues } from "../forms/schema";
import { freshStepId, isDuration, valuesOf, type ShownProblem, type StepDraft } from "../flows/model";
import { useCatalogStore } from "../stores/catalog";
import SchemaForm from "./SchemaForm.vue";

/** One step of the flow editor: an action with a params form, or a wait. */
const props = defineProps<{
  index: number;
  count: number;
  /** The other steps' IDs, for picking a fresh one. */
  otherIds: string[];
  /** Problems under this step, minus its params. */
  problems: ShownProblem[];
  paramErrors: Record<string, string>;
  /** The step's position in the active run, if it's there now. */
  current?: boolean;
}>();
const step = defineModel<StepDraft>({ required: true });
const emit = defineEmits<{ move: [by: -1 | 1]; remove: [] }>();

const catalog = useCatalogStore();
const conn = computed(() => catalog.connections.find((c) => c.id === step.value.conn) ?? null);
const manifest = computed(() => (conn.value === null ? null : catalog.manifest(conn.value.plugin)));
const actions = computed(() => manifest.value?.actions ?? []);
const action = computed(() => actions.value.find((a) => a.id === step.value.action) ?? null);
const fields = computed(() => (action.value === null ? [] : fieldsOf(action.value.params)));
const prefix = computed(() => `step-${step.value.key}`);

// A manifest that loads after the editor opened fills the form from the stored params.
watch(
  fields,
  (next) => {
    if (next.length > 0 && Object.keys(step.value.values).length === 0) {
      step.value.values = valuesOf(next, step.value.params);
    }
  },
  { immediate: true },
);

/** A new target or action starts from that action's defaults, and renames an ID still left as generated. */
function retarget(connId: string, actionId: string) {
  const s = step.value;
  const generated = new RegExp(`^(step|${s.action || "step"})\\d+$`).test(s.id);
  s.conn = connId;
  s.action = actionId;
  s.params = {};
  const schema = catalog.connections.find((c) => c.id === connId);
  const def = schema === undefined ? undefined : catalog.manifest(schema.plugin)?.actions.find((a) => a.id === actionId);
  s.values = def === undefined ? {} : initialValues(fieldsOf(def.params));
  if (generated && actionId !== "") s.id = freshStepId(actionId, props.otherIds);
}

function pickConn(connId: string) {
  const plugin = catalog.connections.find((c) => c.id === connId)?.plugin;
  const list = plugin === undefined ? [] : (catalog.manifest(plugin)?.actions ?? []);
  retarget(connId, list.some((a) => a.id === step.value.action) ? step.value.action : (list[0]?.id ?? ""));
}
</script>

<template>
  <li
    class="step"
    :class="{ disabled: !step.enabled, current }"
    :data-step="step.id"
  >
    <div class="row step-head">
      <span class="step-no">{{ index + 1 }}</span>
      <span class="badge muted">{{ step.kind }}</span>
      <input
        v-model="step.name"
        class="step-name"
        :placeholder="step.kind === 'wait' ? 'Wait' : action?.name ?? 'Action'"
        :aria-label="`Step ${index + 1} name`"
      >
      <label class="muted"><input
        v-model="step.enabled"
        type="checkbox"
      > Enabled</label>
      <span class="spacer" />
      <button
        type="button"
        class="secondary icon"
        :disabled="index === 0"
        :aria-label="`Move step ${index + 1} up`"
        title="Move up"
        @click="emit('move', -1)"
      >
        ↑
      </button>
      <button
        type="button"
        class="secondary icon"
        :disabled="index === count - 1"
        :aria-label="`Move step ${index + 1} down`"
        title="Move down"
        @click="emit('move', 1)"
      >
        ↓
      </button>
      <button
        type="button"
        class="secondary icon"
        :aria-label="`Remove step ${index + 1}`"
        title="Remove"
        @click="emit('remove')"
      >
        ×
      </button>
    </div>

    <div
      v-if="step.kind === 'action'"
      class="row"
    >
      <div class="field">
        <label :for="`${prefix}-conn`">Connection</label>
        <select
          :id="`${prefix}-conn`"
          :value="step.conn"
          @change="pickConn(($event.target as HTMLSelectElement).value)"
        >
          <option
            v-if="conn === null"
            :value="step.conn"
          >
            {{ step.conn || "Pick a connection" }}
          </option>
          <option
            v-for="c in catalog.connections"
            :key="c.id"
            :value="c.id"
          >
            {{ c.alias }}
          </option>
        </select>
      </div>
      <div class="field">
        <label :for="`${prefix}-action`">Action</label>
        <select
          :id="`${prefix}-action`"
          :value="step.action"
          :disabled="manifest === null"
          @change="retarget(step.conn, ($event.target as HTMLSelectElement).value)"
        >
          <option
            v-if="action === null"
            :value="step.action"
          >
            {{ step.action }}
          </option>
          <option
            v-for="a in actions"
            :key="a.id"
            :value="a.id"
          >
            {{ a.name }} ({{ a.id }})
          </option>
        </select>
      </div>
    </div>
    <template v-if="step.kind === 'action'">
      <SchemaForm
        v-if="action !== null"
        v-model="step.values"
        :fields="fields"
        :errors="paramErrors"
        :alias="conn?.alias"
        :manifest="manifest"
        :id-prefix="`${prefix}-param`"
      />
      <p
        v-else-if="step.conn !== '' && manifest === null"
        class="muted"
      >
        This connection's plugin hasn't loaded, so its params can't be edited. Saving keeps them:
        <code>{{ JSON.stringify(step.params) }}</code>
      </p>
    </template>

    <div
      v-else
      class="field"
    >
      <label :for="`${prefix}-for`">Duration</label>
      <input
        :id="`${prefix}-for`"
        v-model="step.for"
        placeholder="1s"
        spellcheck="false"
        autocomplete="off"
      >
      <small
        v-if="step.for.trim() !== '' && !isDuration(step.for.trim())"
        class="error"
      >Write it like 500ms, 2s or 1m30s.</small>
      <small
        v-else
        class="hint"
      >Like 500ms, 2s or 1m30s.</small>
    </div>

    <details class="advanced">
      <summary class="muted">
        ID, timeout and errors
      </summary>
      <div class="row">
        <div class="field">
          <label :for="`${prefix}-id`">Step ID</label>
          <input
            :id="`${prefix}-id`"
            v-model="step.id"
            spellcheck="false"
            autocomplete="off"
          >
        </div>
        <div class="field">
          <label :for="`${prefix}-timeout`">Timeout</label>
          <input
            :id="`${prefix}-timeout`"
            v-model="step.timeout"
            placeholder="None"
            spellcheck="false"
            autocomplete="off"
          >
          <small
            v-if="step.timeout.trim() !== '' && !isDuration(step.timeout.trim())"
            class="error"
          >Write it like 500ms, 2s or 1m30s.</small>
        </div>
        <div class="field">
          <label :for="`${prefix}-onerror`">On error</label>
          <select
            :id="`${prefix}-onerror`"
            v-model="step.onError"
          >
            <option value="fail">
              Fail the run
            </option>
            <option value="continue">
              Continue
            </option>
          </select>
        </div>
      </div>
    </details>

    <ul
      v-if="problems.length > 0"
      class="problems"
    >
      <li
        v-for="(p, i) in problems"
        :key="i"
        :class="p.warning ? 'warn' : 'error'"
      >
        {{ p.message }}
      </li>
    </ul>
  </li>
</template>

<style scoped>
.step {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 0.75rem;
  background: var(--bg);
}

.step.disabled {
  opacity: 0.65;
}

.step.current {
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent);
}

.step-head {
  margin-bottom: 0.5rem;
}

.step-no {
  font-weight: 600;
  min-width: 1.25rem;
}

.step-name {
  flex: 1;
  min-width: 10rem;
  max-width: 24rem;
}

.spacer {
  flex: 1;
}

button.icon {
  padding: 0.15rem 0.55rem;
}

.advanced summary {
  cursor: pointer;
  margin-bottom: 0.5rem;
}
</style>
