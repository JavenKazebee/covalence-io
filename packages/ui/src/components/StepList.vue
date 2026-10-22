<script setup lang="ts">
import {
  freshStepId,
  newStepDraft,
  type PathSegment,
  type ShownProblem,
  type StepContext,
  type StepDraft,
  type StepKind,
} from "../flows/model";
import { useCatalogStore } from "../stores/catalog";
import StepEditor from "./StepEditor.vue";

/**
 * A list of steps that run in order: the flow's body, or a `branch` arm.
 * `path` is where the list sits in the flow, for the server's problems.
 */
const props = defineProps<{
  path: PathSegment[];
  problems: ShownProblem[];
  /** Param errors found before sending, by step key. */
  localErrors: Map<number, Record<string, string>>;
  ctx: StepContext;
}>();
const steps = defineModel<StepDraft[]>({ required: true });
/** The list changed shape, so the server's paths below `path` no longer line up. */
const emit = defineEmits<{ reshape: [path: PathSegment[]] }>();

const catalog = useCatalogStore();

const kinds: { kind: StepKind; label: string }[] = [
  { kind: "action", label: "Action" },
  { kind: "wait", label: "Wait" },
  { kind: "waitUntil", label: "Wait until" },
  { kind: "set", label: "Set" },
  { kind: "branch", label: "Branch" },
];

function add(kind: StepKind) {
  const taken = props.ctx.all;
  if (kind !== "action") {
    steps.value.push(newStepDraft(kind, freshStepId(kind === "waitUntil" ? "until" : kind, taken)));
    return;
  }
  // Start from the last action's target, the usual next step.
  const last = [...steps.value].reverse().find((s) => s.kind === "action");
  const conn = last?.conn ?? catalog.connections[0]?.id ?? "";
  const plugin = catalog.connections.find((c) => c.id === conn)?.plugin;
  const action = last?.action ?? (plugin === undefined ? undefined : catalog.manifest(plugin)?.actions[0]?.id) ?? "";
  const step = newStepDraft("action", freshStepId(action || "step", taken));
  step.conn = conn;
  step.action = action;
  steps.value.push(step);
}

function move(index: number, by: -1 | 1) {
  const [step] = steps.value.splice(index, 1);
  if (step !== undefined) steps.value.splice(index + by, 0, step);
  emit("reshape", props.path);
}

function remove(index: number) {
  steps.value.splice(index, 1);
  emit("reshape", props.path);
}
</script>

<template>
  <ol class="steps">
    <StepEditor
      v-for="(step, i) in steps"
      :key="step.key"
      v-model="steps[i]!"
      :index="i"
      :count="steps.length"
      :path="[...path, i]"
      :problems="problems"
      :local-errors="localErrors"
      :ctx="ctx"
      @move="(by) => move(i, by)"
      @remove="remove(i)"
      @reshape="(p) => emit('reshape', p)"
    />
  </ol>
  <p
    v-if="steps.length === 0"
    class="muted"
  >
    No steps yet.
  </p>
  <div class="row add-step">
    <span class="muted">Add</span>
    <button
      v-for="k in kinds"
      :key="k.kind"
      type="button"
      class="secondary"
      :disabled="k.kind === 'action' && catalog.connections.length === 0"
      @click="add(k.kind)"
    >
      {{ k.label }}
    </button>
  </div>
</template>

<style scoped>
.steps {
  list-style: none;
  padding: 0;
  margin: 0 0 0.75rem;
  display: grid;
  gap: 0.5rem;
}

.add-step {
  gap: 0.4rem;
}

.add-step button {
  padding: 0.2rem 0.6rem;
}
</style>
