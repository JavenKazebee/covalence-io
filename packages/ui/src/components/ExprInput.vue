<script setup lang="ts">
import type { EditorView } from "@codemirror/view";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { api } from "../api";
import { stepWarnings } from "../flows/expr";
import type { Evaluated, ShownProblem } from "../flows/model";

/**
 * A JavaScript expression field (flow spec §8.7, ADR 0004): one line that
 * grows as you type, with a live preview from `POST /expressions/evaluate`,
 * the server's problems, and warnings for `steps.<id>` that won't be there.
 */
const props = withDefaults(
  defineProps<{
    label: string;
    placeholder?: string;
    /** The server's problems for this field. */
    problems?: ShownProblem[];
    /** Steps finished when this runs, or null to skip step checks. */
    known?: ReadonlySet<string> | null;
    /** Every step ID in the flow. */
    all?: ReadonlySet<string>;
    /** A sample `trigger` for the preview. */
    trigger?: unknown;
  }>(),
  { placeholder: "", problems: () => [], known: null, all: () => new Set(), trigger: undefined },
);
const source = defineModel<string>({ required: true });

const host = ref<HTMLDivElement | null>(null);
let view: EditorView | null = null;
let unmounted = false;

// CodeMirror loads with the first expression field, not with the app.
onMounted(async () => {
  const [{ defaultKeymap, history, historyKeymap }, { javascript }, { EditorState }, cm] = await Promise.all([
    import("@codemirror/commands"),
    import("@codemirror/lang-javascript"),
    import("@codemirror/state"),
    import("@codemirror/view"),
  ]);
  if (unmounted || host.value === null) return;
  const { EditorView, keymap, placeholder: placeholderText } = cm;
  view = new EditorView({
    parent: host.value,
    state: EditorState.create({
      doc: source.value,
      extensions: [
        history(),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        javascript(),
        EditorView.lineWrapping,
        placeholderText(props.placeholder),
        EditorView.contentAttributes.of({ "aria-label": props.label, spellcheck: "false" }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) source.value = update.state.doc.toString();
        }),
      ],
    }),
  });
});
onBeforeUnmount(() => {
  unmounted = true;
  view?.destroy();
});

// A change from outside (a reload, a revert) replaces the text.
watch(source, (text) => {
  if (view !== null && view.state.doc.toString() !== text) {
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
  }
});

// ---- Preview ----

const preview = ref<Evaluated | null>(null);
const previewing = ref(false);
let timer: ReturnType<typeof setTimeout> | undefined;
let asked = 0;

watch(
  () => [source.value, JSON.stringify(props.trigger ?? null)] as const,
  ([text]) => {
    clearTimeout(timer);
    if (text.trim() === "") {
      preview.value = null;
      return;
    }
    timer = setTimeout(async () => {
      const mine = ++asked;
      previewing.value = true;
      try {
        const result = await api.evaluate(text, props.trigger);
        if (mine === asked) preview.value = result;
      } catch {
        if (mine === asked) preview.value = null;
      } finally {
        if (mine === asked) previewing.value = false;
      }
    }, 350);
  },
  { immediate: true },
);
onBeforeUnmount(() => clearTimeout(timer));

function shown(value: unknown): string {
  const text = JSON.stringify(value) ?? String(value);
  return text.length > 80 ? `${text.slice(0, 79)}…` : text;
}

const previewText = computed(() => {
  const p = preview.value;
  if (p === null) return null;
  if (p.error !== undefined) return null;
  if ("value" in p) return `= ${shown(p.value)}`;
  // The preview has no run, so these read as undefined there.
  const runOnly = /\b(local|steps|trigger)\b/.test(source.value);
  return runOnly ? "= undefined here: trigger, local and steps only exist in a run" : "= undefined";
});
const reads = computed(() => (preview.value?.reads.length ? `Reads ${preview.value.reads.join(", ")}` : "Reads no state"));

const warnings = computed(() => (props.known === null ? [] : stepWarnings(source.value, props.known, props.all)));
const where = (at: { line: number; column: number } | undefined) => (at === undefined ? "" : ` (line ${at.line}, column ${at.column})`);
</script>

<template>
  <div class="expr">
    <div
      ref="host"
      class="expr-input"
      :class="{ invalid: problems.some((p) => !p.warning) }"
    />
    <small
      v-if="previewText"
      class="preview muted"
      :title="reads"
      data-testid="preview"
    >{{ previewText }}</small>
    <small
      v-else-if="preview?.error && problems.length === 0"
      class="preview warn"
      data-testid="preview"
    >Now: {{ preview.error.message }}{{ where(preview.error.at) }}</small>
    <small
      v-for="(w, i) in warnings"
      :key="`w${i}`"
      class="warn"
    >{{ w }}</small>
    <small
      v-for="(p, i) in problems"
      :key="`p${i}`"
      :class="p.warning ? 'warn' : 'error'"
    >{{ p.message }}{{ where(p.at) }}</small>
  </div>
</template>

<style scoped>
.expr {
  display: grid;
  gap: 0.15rem;
  min-width: 12rem;
  flex: 1;
}

.expr-input {
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--panel);
}

.expr-input:focus-within {
  border-color: var(--accent);
}

.expr-input.invalid {
  border-color: var(--bad);
}

.expr-input :deep(.cm-editor) {
  font-family: var(--mono);
  font-size: 0.92em;
  outline: none;
}

.expr-input :deep(.cm-content) {
  padding: 0.3rem 0.1rem;
  caret-color: var(--text);
}

.expr-input :deep(.cm-placeholder) {
  color: var(--muted);
}

.preview {
  font-family: var(--mono);
}

.warn {
  color: var(--warn);
}
</style>
