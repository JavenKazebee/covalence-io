<script setup lang="ts">
import type { Manifest } from "../api";
import type { Field } from "../forms/schema";
import ChoiceSelect from "./ChoiceSelect.vue";
import ExprInput from "./ExprInput.vue";

/**
 * Inputs for a flat JSON Schema object (`forms/schema.ts`). Values stay as
 * input text until `parseValues` turns them into JSON. With `exprs`, each
 * field can be given as an expression instead (flow spec §8.3).
 */
const props = defineProps<{
  fields: Field[];
  errors?: Record<string, string>;
  /** Fills `x-choices` fields from this connection's live state. */
  alias?: string;
  manifest?: Manifest | null;
  /** Keeps input IDs unique when there are several forms. */
  idPrefix: string;
  /** For expression fields: the steps finished before this one, and all of them. */
  known?: ReadonlySet<string> | null;
  all?: ReadonlySet<string>;
}>();
const values = defineModel<Record<string, string | boolean>>({ required: true });
/** Fields given as expressions, by key; absent when the form doesn't offer them. */
const exprs = defineModel<Record<string, string> | undefined>("exprs", { default: undefined });

function labelField(from: string): string | undefined {
  const entry = props.manifest?.state[from];
  return entry !== undefined && "collection" in entry ? entry.collection.label : undefined;
}

const inputId = (field: Field) => `${props.idPrefix}-${field.key}`;
const isExpr = (field: Field) => exprs.value !== undefined && field.key in exprs.value;

/** Switches a field between its input and an expression, carrying the value across. */
function toggle(field: Field) {
  const map = exprs.value;
  if (map === undefined) return;
  if (field.key in map) {
    delete map[field.key];
    return;
  }
  const v = values.value[field.key];
  map[field.key] =
    v === undefined || v === "" ? "" : field.kind === "text" || field.kind === "enum" ? JSON.stringify(v) : String(v);
}
</script>

<template>
  <div
    v-for="field in fields"
    :key="field.key"
    class="field"
  >
    <div class="label-row">
      <label :for="inputId(field)">
        {{ field.label }}<span
          v-if="field.required"
          class="required"
          title="Required"
        >*</span>
      </label>
      <button
        v-if="exprs !== undefined"
        type="button"
        class="secondary mode"
        :class="{ on: isExpr(field) }"
        :aria-pressed="isExpr(field)"
        :aria-label="`${field.label} as an expression`"
        :title="isExpr(field) ? 'Use a value' : 'Use an expression'"
        @click="toggle(field)"
      >
        ƒx
      </button>
    </div>
    <ExprInput
      v-if="isExpr(field)"
      v-model="exprs![field.key]!"
      :label="`${field.label} expression`"
      placeholder="vars.nextSource"
      :known="known ?? null"
      :all="all"
    />
    <input
      v-else-if="field.kind === 'boolean'"
      :id="inputId(field)"
      v-model="values[field.key]"
      type="checkbox"
    >
    <select
      v-else-if="field.kind === 'enum'"
      :id="inputId(field)"
      v-model="values[field.key]"
    >
      <option
        v-if="!field.required"
        value=""
      />
      <option
        v-for="option in field.schema.enum"
        :key="String(option)"
        :value="String(option)"
      >
        {{ option }}
      </option>
    </select>
    <ChoiceSelect
      v-else-if="field.kind === 'choices' && alias !== undefined && field.from !== undefined"
      :id="inputId(field)"
      v-model="values[field.key]!"
      :path="`${alias}.${field.from}`"
      :label-field="labelField(field.from)"
    />
    <input
      v-else
      :id="inputId(field)"
      v-model="values[field.key]"
      :inputmode="field.kind === 'text' ? 'text' : 'numeric'"
      :placeholder="field.schema.default === undefined ? '' : String(field.schema.default)"
      spellcheck="false"
      autocomplete="off"
    >
    <small
      v-if="field.schema.description"
      class="hint"
    >{{ field.schema.description }}</small>
    <small
      v-if="errors?.[field.key]"
      class="error"
    >{{ errors[field.key] }}</small>
  </div>
</template>

<style scoped>
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
</style>
