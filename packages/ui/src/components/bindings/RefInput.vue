<script setup lang="ts">
import { ref, watch } from "vue";
import { useAliases, useValidity } from "../../bindings/editing";
import { parseRef, refText, type Ref } from "../../bindings/model";

/** A state path in text form, `vh.outputs[1].source`, stored as a `Ref` by connection ID. */
defineProps<{ label: string; placeholder?: string }>();
const model = defineModel<Ref>({ required: true });

const aliases = useAliases();
const setValid = useValidity();
const text = ref(refText(model.value, aliases.value));
const error = ref<string | null>(null);

// A change from outside, or aliases loading, rewrites the text unless it's what the text says.
watch(
  () => [JSON.stringify(model.value), aliases.value] as const,
  () => {
    const parsed = parseRef(text.value, aliases.value);
    if (parsed.ok && JSON.stringify(parsed.value) === JSON.stringify(model.value)) return;
    text.value = refText(model.value, aliases.value);
    error.value = null;
    setValid(true);
  },
);

function input(value: string) {
  text.value = value;
  const parsed = parseRef(value, aliases.value);
  error.value = parsed.ok ? null : parsed.error;
  setValid(parsed.ok);
  if (parsed.ok) model.value = parsed.value;
}
</script>

<template>
  <span class="ref-input">
    <input
      :value="text"
      :aria-label="label"
      :placeholder="placeholder ?? 'vh.outputs[1].source'"
      :class="{ invalid: error }"
      spellcheck="false"
      autocomplete="off"
      @input="input(($event.target as HTMLInputElement).value)"
    >
    <small
      v-if="error"
      class="error"
    >{{ error }}</small>
  </span>
</template>

<style scoped>
.ref-input {
  display: inline-grid;
  gap: 0.15rem;
}

input {
  font-family: var(--mono);
  font-size: 0.92em;
  min-width: 14rem;
}

input.invalid {
  border-color: var(--bad);
}
</style>
