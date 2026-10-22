<script setup lang="ts">
import { ref, watch } from "vue";
import { useAliases, useValidity } from "../../bindings/editing";
import { parseTemplate, templateText, type TemplatePart } from "../../bindings/model";

/** A template's text: `Out 1: {vh.outputs[1].source}`, `{path|mmss}` for a format. */
defineProps<{ label: string }>();
const model = defineModel<TemplatePart[]>({ required: true });

const aliases = useAliases();
const setValid = useValidity();
const text = ref(templateText(model.value, aliases.value));
const error = ref<string | null>(null);

watch(
  () => [JSON.stringify(model.value), aliases.value] as const,
  () => {
    const parsed = parseTemplate(text.value, aliases.value);
    if (parsed.ok && JSON.stringify(parsed.value) === JSON.stringify(model.value)) return;
    text.value = templateText(model.value, aliases.value);
    error.value = null;
    setValid(true);
  },
);

function input(value: string) {
  text.value = value;
  const parsed = parseTemplate(value, aliases.value);
  error.value = parsed.ok ? null : parsed.error;
  setValid(parsed.ok);
  if (parsed.ok) model.value = parsed.value;
}
</script>

<template>
  <span class="template-input">
    <input
      :value="text"
      :aria-label="label"
      placeholder="Out 1: {vh.outputs[1].source}"
      :class="{ invalid: error }"
      spellcheck="false"
      autocomplete="off"
      @input="input(($event.target as HTMLInputElement).value)"
    >
    <small
      v-if="error"
      class="error"
    >{{ error }}</small>
    <small
      v-else
      class="hint"
    >Values go in braces, with an optional format: <code>{vh.timer|mmss}</code>. <code>{{ "{{" }}</code> is a brace.</small>
  </span>
</template>

<style scoped>
.template-input {
  display: grid;
  gap: 0.15rem;
  flex: 1;
  min-width: 16rem;
}

input {
  font-family: var(--mono);
  font-size: 0.92em;
}

input.invalid {
  border-color: var(--bad);
}
</style>
