<script setup lang="ts">
import type { Manifest } from "../api";
import type { Field } from "../forms/schema";
import ChoiceSelect from "./ChoiceSelect.vue";

/**
 * Inputs for a flat JSON Schema object (`forms/schema.ts`). Values stay as
 * input text until `parseValues` turns them into JSON.
 */
const props = defineProps<{
  fields: Field[];
  errors?: Record<string, string>;
  /** Fills `x-choices` fields from this connection's live state. */
  alias?: string;
  manifest?: Manifest | null;
  /** Keeps input IDs unique when there are several forms. */
  idPrefix: string;
}>();
const values = defineModel<Record<string, string | boolean>>({ required: true });

function labelField(from: string): string | undefined {
  const entry = props.manifest?.state[from];
  return entry !== undefined && "collection" in entry ? entry.collection.label : undefined;
}

const inputId = (field: Field) => `${props.idPrefix}-${field.key}`;
</script>

<template>
  <div
    v-for="field in fields"
    :key="field.key"
    class="field"
  >
    <label :for="inputId(field)">
      {{ field.label }}<span
        v-if="field.required"
        class="required"
        title="Required"
      >*</span>
    </label>
    <input
      v-if="field.kind === 'boolean'"
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
