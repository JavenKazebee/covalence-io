<script setup lang="ts">
import { computed, watch } from "vue";
import { stepsBefore } from "../flows/expr";
import {
  freshStepId,
  isDuration,
  newArmDraft,
  paramProblems,
  problemsAt,
  valuesOf,
  type PathSegment,
  type ShownProblem,
  type StepContext,
  type StepDraft,
} from "../flows/model";
import { fieldsOf, initialValues } from "../forms/schema";
import { useCatalogStore } from "../stores/catalog";
import ExprInput from "./ExprInput.vue";
import SchemaForm from "./SchemaForm.vue";
import StepList from "./StepList.vue";

/**
 * One step of the flow editor (flow spec §8.3): an action with a params form,
 * a wait, a wait-until, a set, or a branch with its own step lists.
 */
const props = defineProps<{
  index: number;
  count: number;
  /** Where the step sits in the flow: `["body", 1]`, `["body", 1, "arms", 0, "steps", 2]`. */
  path: PathSegment[];
  /** Every problem in the flow; the step picks its own. */
  problems: ShownProblem[];
  localErrors: Map<number, Record<string, string>>;
  ctx: StepContext;
}>();
const step = defineModel<StepDraft>({ required: true });
const emit = defineEmits<{ move: [by: -1 | 1]; remove: []; reshape: [path: PathSegment[]] }>();

const catalog = useCatalogStore();
const conn = computed(() => catalog.connections.find((c) => c.id === step.value.conn) ?? null);
const manifest = computed(() => (conn.value === null ? null : catalog.manifest(conn.value.plugin)));
const actions = computed(() => manifest.value?.actions ?? []);
const action = computed(() => actions.value.find((a) => a.id === step.value.action) ?? null);
const fields = computed(() => (action.value === null ? [] : fieldsOf(action.value.params)));
const prefix = computed(() => `step-${step.value.key}`);

const kindLabel: Record<StepDraft["kind"], string> = {
  action: "action",
  wait: "wait",
  waitUntil: "wait until",
  set: "set",
  branch: "branch",
};

/** Steps an expression here can read, as `steps.<id>`. */
const known = computed(() => stepsBefore(props.ctx.body, step.value));
const current = computed(() => props.ctx.position.includes(step.value.id));

const at = (...rest: PathSegment[]) => problemsAt(props.problems, [...props.path, ...rest]);
/** Problems for the step itself: not its fields shown elsewhere, nor its nested steps. */
const ownProblems = computed(() =>
  problemsAt(
    props.problems,
    props.path,
    ["params", "for", "until", "path", "value", "arms", "else"].map((k) => [...props.path, k]),
  ),
);
const paramErrors = computed(() => ({
  ...paramProblems(props.problems, props.path),
  ...props.localErrors.get(step.value.key),
}));

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
  s.exprs = {};
  const schema = catalog.connections.find((c) => c.id === connId);
  const def = schema === undefined ? undefined : catalog.manifest(schema.plugin)?.actions.find((a) => a.id === actionId);
  s.values = def === undefined ? {} : initialValues(fieldsOf(def.params));
  if (generated && actionId !== "") s.id = freshStepId(actionId, [...props.ctx.all].filter((id) => id !== s.id));
}

function pickConn(connId: string) {
  const plugin = catalog.connections.find((c) => c.id === connId)?.plugin;
  const list = plugin === undefined ? [] : (catalog.manifest(plugin)?.actions ?? []);
  retarget(connId, list.some((a) => a.id === step.value.action) ? step.value.action : (list[0]?.id ?? ""));
}

// ---- wait ----

function toggleWaitExpr() {
  const s = step.value;
  if (s.forExpr === null) {
    s.forExpr = s.for.trim() === "" ? "" : JSON.stringify(s.for.trim());
  } else {
    s.forExpr = null;
    if (!isDuration(s.for.trim())) s.for = "1s";
  }
}

// ---- set ----

const setRoot = computed({
  get: () => (step.value.path.startsWith("vars.") ? "vars" : "local"),
  set: (root: string) => (step.value.path = `${root}.${setName.value}`),
});
const setName = computed({
  get: () => step.value.path.replace(/^(local|vars)\./, ""),
  set: (name: string) => (step.value.path = `${setRoot.value}.${name.trim()}`),
});

// ---- branch ----

function addArm() {
  step.value.arms.push(newArmDraft());
}

function removeArm(index: number) {
  step.value.arms.splice(index, 1);
  emit("reshape", props.path);
}

function toggleElse() {
  step.value.otherwise = step.value.otherwise === null ? [] : null;
  emit("reshape", props.path);
}
</script>

<template>
  <li
    class="step"
    :class="{ disabled: !step.enabled, current, block: step.kind === 'branch' }"
    :data-step="step.id"
  >
    <div class="row step-head">
      <span class="step-no">{{ index + 1 }}</span>
      <span class="badge muted">{{ kindLabel[step.kind] }}</span>
      <input
        v-model="step.name"
        class="step-name"
        :placeholder="step.kind === 'action' ? action?.name ?? 'Action' : step.id"
        :aria-label="`Step ${step.id} name`"
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
        :aria-label="`Move step ${step.id} up`"
        title="Move up"
        @click="emit('move', -1)"
      >
        ↑
      </button>
      <button
        type="button"
        class="secondary icon"
        :disabled="index === count - 1"
        :aria-label="`Move step ${step.id} down`"
        title="Move down"
        @click="emit('move', 1)"
      >
        ↓
      </button>
      <button
        type="button"
        class="secondary icon"
        :aria-label="`Remove step ${step.id}`"
        title="Remove"
        @click="emit('remove')"
      >
        ×
      </button>
    </div>

    <template v-if="step.kind === 'action'">
      <div class="row">
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
      <SchemaForm
        v-if="action !== null"
        v-model="step.values"
        v-model:exprs="step.exprs"
        :fields="fields"
        :errors="paramErrors"
        :alias="conn?.alias"
        :manifest="manifest"
        :id-prefix="`${prefix}-param`"
        :known="known"
        :all="ctx.all"
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
      v-else-if="step.kind === 'wait'"
      class="field wide"
    >
      <div class="label-row">
        <label :for="`${prefix}-for`">Duration</label>
        <button
          type="button"
          class="secondary mode"
          :class="{ on: step.forExpr !== null }"
          :aria-pressed="step.forExpr !== null"
          aria-label="Duration as an expression"
          :title="step.forExpr === null ? 'Use an expression' : 'Use a duration'"
          @click="toggleWaitExpr"
        >
          ƒx
        </button>
      </div>
      <template v-if="step.forExpr === null">
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
      </template>
      <template v-else>
        <ExprInput
          v-model="step.forExpr"
          label="Duration expression"
          placeholder="vars.pauseMs"
          :problems="at('for')"
          :known="known"
          :all="ctx.all"
        />
        <small class="hint">A duration like "2s", or milliseconds.</small>
      </template>
    </div>

    <div
      v-else-if="step.kind === 'waitUntil'"
      class="field wide"
    >
      <label>Until</label>
      <ExprInput
        v-model="step.until"
        label="Until"
        placeholder="vh.outputs[1].source == 3"
        :problems="at('until')"
        :known="known"
        :all="ctx.all"
      />
      <small class="hint">Done as soon as this is true: straight away, or when what it reads changes.</small>
    </div>

    <div
      v-else-if="step.kind === 'set'"
      class="row set"
    >
      <div class="field">
        <label :for="`${prefix}-path`">Set</label>
        <div class="row path">
          <select
            v-model="setRoot"
            aria-label="Local or variable"
          >
            <option value="local">
              local
            </option>
            <option value="vars">
              vars
            </option>
          </select>
          <span>.</span>
          <input
            :id="`${prefix}-path`"
            v-model="setName"
            placeholder="name"
            spellcheck="false"
            autocomplete="off"
          >
        </div>
        <small class="hint">{{ setRoot === "vars" ? "A user variable, readable everywhere." : "Lasts for this run." }}</small>
        <small
          v-for="(p, i) in at('path')"
          :key="i"
          class="error"
        >{{ p.message }}</small>
      </div>
      <div class="field wide">
        <label>To</label>
        <ExprInput
          v-model="step.value"
          label="Value"
          placeholder="steps.route1.result"
          :problems="at('value')"
          :known="known"
          :all="ctx.all"
        />
      </div>
    </div>

    <div
      v-else-if="step.kind === 'branch'"
      class="branch"
    >
      <div
        v-for="(arm, a) in step.arms"
        :key="arm.key"
        class="arm"
        :data-arm="a"
      >
        <div class="row arm-head">
          <strong>{{ a === 0 ? "If" : "Else if" }}</strong>
          <ExprInput
            v-model="arm.when"
            :label="`Arm ${a + 1} condition`"
            placeholder="vars.mode == 'live'"
            :problems="at('arms', a, 'when')"
            :known="known"
            :all="ctx.all"
          />
          <button
            type="button"
            class="secondary icon"
            :aria-label="`Remove arm ${a + 1}`"
            title="Remove"
            @click="removeArm(a)"
          >
            ×
          </button>
        </div>
        <StepList
          v-model="arm.steps"
          :path="[...path, 'arms', a, 'steps']"
          :problems="problems"
          :local-errors="localErrors"
          :ctx="ctx"
          @reshape="(p) => emit('reshape', p)"
        />
      </div>
      <div
        v-if="step.otherwise !== null"
        class="arm"
      >
        <div class="row arm-head">
          <strong>Else</strong>
          <span class="spacer" />
          <button
            type="button"
            class="secondary icon"
            aria-label="Remove else"
            title="Remove"
            @click="toggleElse"
          >
            ×
          </button>
        </div>
        <StepList
          v-model="step.otherwise"
          :path="[...path, 'else']"
          :problems="problems"
          :local-errors="localErrors"
          :ctx="ctx"
          @reshape="(p) => emit('reshape', p)"
        />
      </div>
      <div class="row">
        <button
          type="button"
          class="secondary"
          @click="addArm"
        >
          Add else if
        </button>
        <button
          v-if="step.otherwise === null"
          type="button"
          class="secondary"
          @click="toggleElse"
        >
          Add else
        </button>
      </div>
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
          <small class="hint">Read later as <code>steps.{{ step.id || "…" }}</code>.</small>
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
      v-if="ownProblems.length > 0"
      class="problems"
    >
      <li
        v-for="(p, i) in ownProblems"
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

.step.block {
  background: var(--panel);
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

.field.wide {
  max-width: 40rem;
  flex: 1;
}

.label-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}

button.mode {
  padding: 0 0.4rem;
  font-size: 0.85em;
  font-style: italic;
}

button.mode.on {
  background: var(--accent);
  color: #fff;
}

.set {
  align-items: start;
}

.path {
  gap: 0.25rem;
  flex-wrap: nowrap;
}

.path input {
  min-width: 6rem;
}

.branch {
  display: grid;
  gap: 0.5rem;
  margin-bottom: 0.5rem;
}

.arm {
  border-left: 3px solid var(--border);
  padding-left: 0.75rem;
}

.arm-head {
  align-items: start;
  margin-bottom: 0.5rem;
}

.arm-head strong {
  padding-top: 0.3rem;
  min-width: 3.5rem;
}

.advanced summary {
  cursor: pointer;
  margin-bottom: 0.5rem;
}
</style>
