<script setup lang="ts">
import { computed, ref } from "vue";
import { api, ApiError } from "../../api";
import { provideInvalid } from "../../bindings/editing";
import { fromDraft, newDraft, props as allProps, toDraft, type ControlBinding, type RuntimeProblem } from "../../bindings/model";
import { usePreviews } from "../../bindings/usePreviews";
import { firesOn, newFlowDraft, type ShownProblem } from "../../flows/model";
import { navigate } from "../../nav";
import { useFlowsStore } from "../../stores/flows";
import type { Page } from "../../surfaces/model";
import PropEditor from "./PropEditor.vue";

/**
 * The bindings of one key (bindings spec §1): a kind per property, previewed
 * against live state as the core would draw it. Saving with nothing bound
 * deletes the binding.
 */
const props = defineProps<{
  page: Page;
  control: string;
  stored: ControlBinding | undefined;
  runtime: RuntimeProblem[];
}>();
const emit = defineEmits<{ saved: [] }>();

const flows = useFlowsStore();
const invalid = provideInvalid();

// Mounted per key (keyed by the page editor), so the draft starts from what's stored.
const draft = ref<ControlBinding>(props.stored === undefined ? newDraft(props.page.id, props.control) : toDraft(props.stored));
const problems = ref<ShownProblem[]>([]);
const saving = ref(false);
const saveError = ref<string | null>(null);
const savedNote = ref<string | null>(null);

const sending = computed(() => fromDraft(draft.value));
const { results } = usePreviews(() => [sending.value], 250);
const result = computed(() => results.get(props.control));
const preview = computed(() => result.value?.preview);

const empty = computed(() => Object.keys(sending.value.props).length === 0);
const baseline = computed(() => JSON.stringify(props.stored === undefined ? newDraft(props.page.id, props.control) : fromDraft(props.stored)));
const dirty = computed(() => JSON.stringify(sending.value) !== baseline.value);

const runtimeOf = (prop: string) => props.runtime.find((p) => p.prop === prop)?.message;
const problemsOf = (prop: string) => problems.value.filter((p) => p.path[0] === "props" && p.path[1] === prop);
/** Problems no property shows: the page, the control, the binding as a whole. */
const otherProblems = computed(() => [
  ...problems.value.filter((p) => !(p.path[0] === "props" && allProps.includes(p.path[1] as never))),
  ...(result.value?.problems ?? []).map((p) => ({ ...p, warning: false })),
]);

async function save() {
  saveError.value = null;
  savedNote.value = null;
  if (invalid.size > 0) {
    saveError.value = "Fix the highlighted fields first.";
    return;
  }
  saving.value = true;
  try {
    if (empty.value) {
      if (props.stored !== undefined) await api.deleteBinding(props.stored.id);
      draft.value = newDraft(props.page.id, props.control);
      problems.value = [];
      savedNote.value = "Unbound.";
    } else {
      const saved = sending.value.id === "" ? await api.createBinding(sending.value) : await api.updateBinding(sending.value);
      draft.value = toDraft(saved.binding);
      problems.value = saved.warnings.map((p) => ({ ...p, warning: true }));
      savedNote.value = saved.warnings.length > 0 ? "Saved, with warnings." : "Saved.";
    }
    emit("saved");
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

function revert() {
  draft.value = props.stored === undefined ? newDraft(props.page.id, props.control) : toDraft(props.stored);
  problems.value = [];
  saveError.value = null;
  savedNote.value = null;
}

// ---- When pressed ----

/** The flow that runs when this key is pressed, if there is one. */
const pressFlow = computed(() => flows.flows.find((f) => f.triggers.some((t) => firesOn(t, props.page.id, props.control, "press"))));

function whenPressed() {
  if (pressFlow.value !== undefined) {
    navigate({ tab: "flows", flow: pressFlow.value.id });
    return;
  }
  // A Button-view flow: this key's press, then steps in order.
  const flow = newFlowDraft();
  const label = preview.value?.look.label.trim();
  flow.name = `${props.page.name} ${props.control}${label ? ` (${label})` : ""}`;
  flow.triggers = [{ kind: "gesture", control: { page: props.page.id, control: props.control }, gesture: "press" }];
  flows.prefill = flow;
  navigate({ tab: "flows", flow: "new" });
}
</script>

<template>
  <form
    class="binding-editor"
    novalidate
    @submit.prevent="save"
  >
    <div class="head">
      <div
        class="key-preview"
        :class="{ stale: preview?.look.stale }"
      >
        <img
          v-if="preview"
          :src="`data:image/png;base64,${preview.image}`"
          :alt="`Preview of ${control}`"
          data-testid="key-preview"
        >
      </div>
      <div class="head-text">
        <h3>Key {{ control }}</h3>
        <p class="muted">
          {{ preview?.look.stale ? "Dimmed: it reads stale state. " : "" }}The preview is live: it's drawn by the core
          from current state, exactly as a surface would show it.
        </p>
        <button
          type="button"
          class="secondary"
          data-testid="when-pressed"
          @click="whenPressed"
        >
          {{ pressFlow ? `When pressed: ${pressFlow.name}` : "When pressed…" }}
        </button>
      </div>
    </div>

    <PropEditor
      v-for="prop in allProps"
      :key="prop"
      v-model="draft.props[prop]"
      :prop="prop"
      :shown="preview?.props[prop]"
      :problems="problemsOf(prop)"
      :runtime="dirty ? undefined : runtimeOf(prop)"
    />

    <ul
      v-if="otherProblems.length > 0"
      class="problems"
    >
      <li
        v-for="(p, i) in otherProblems"
        :key="i"
        :class="p.warning ? 'warn' : 'error'"
      >
        {{ p.message }}
      </li>
    </ul>
    <p
      v-if="result?.error"
      class="error"
    >
      Preview failed: {{ result.error }}
    </p>

    <div class="row save-bar">
      <button
        type="submit"
        :disabled="saving || !dirty"
      >
        {{ saving ? "Saving…" : empty && stored !== undefined ? "Unbind" : "Save" }}
      </button>
      <button
        type="button"
        class="secondary"
        :disabled="!dirty"
        @click="revert"
      >
        Revert
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
      <span
        v-else-if="dirty"
        class="muted"
      >Not saved yet.</span>
    </div>
  </form>
</template>

<style scoped>
.head {
  display: flex;
  gap: 1rem;
  align-items: flex-start;
  margin-bottom: 0.75rem;
}

.head-text {
  flex: 1;
}

.head-text p {
  margin: 0 0 0.5rem;
}

.key-preview {
  width: 96px;
  height: 96px;
  flex: none;
  border-radius: 12px;
  overflow: hidden;
  background: #000;
  box-shadow: 0 0 0 1px var(--border);
}

.key-preview img {
  display: block;
  width: 100%;
  height: 100%;
}

.save-bar {
  position: sticky;
  bottom: 0;
  padding: 0.6rem 0;
  background: var(--panel);
  border-top: 1px solid var(--border);
}
</style>
