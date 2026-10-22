<script setup lang="ts">
import { computed } from "vue";
import { fullHex, type Value } from "../../bindings/model";

/**
 * A literal a property shows: text for a label, or a colour with a picker.
 * The stored value is only replaced when it's edited, so a number stays a
 * number until then.
 */
const props = defineProps<{ type: "text" | "color"; label: string; placeholder?: string; optional?: boolean }>();
const model = defineModel<Value | undefined>({ required: true });

const text = computed(() => (model.value === undefined || model.value === null ? "" : String(model.value)));
const hex = computed(() => fullHex(text.value) ?? "#000000");
const bad = computed(() => props.type === "color" && text.value !== "" && fullHex(text.value) === null);

function set(value: string) {
  model.value = props.optional && value === "" ? undefined : value;
}
</script>

<template>
  <span class="value-field">
    <input
      v-if="type === 'color'"
      type="color"
      class="swatch"
      :value="hex"
      :aria-label="`${label} picker`"
      @input="set(($event.target as HTMLInputElement).value)"
    >
    <input
      :value="text"
      :aria-label="label"
      :placeholder="placeholder ?? (type === 'color' ? '#rrggbb' : '')"
      :class="{ color: type === 'color', invalid: bad }"
      :title="bad ? 'A colour is #rrggbb or #rgb' : ''"
      spellcheck="false"
      autocomplete="off"
      @input="set(($event.target as HTMLInputElement).value)"
    >
  </span>
</template>

<style scoped>
.value-field {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
}

.swatch {
  width: 2rem;
  height: 2rem;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: none;
  cursor: pointer;
}

input.color {
  width: 7rem;
  font-family: var(--mono);
  font-size: 0.92em;
}

input.invalid {
  border-color: var(--bad);
}
</style>
