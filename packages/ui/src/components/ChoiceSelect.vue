<script setup lang="ts">
import { computed } from "vue";
import { collectionChoices } from "../forms/schema";
import { useLiveState } from "../live/useLiveState";

const props = defineProps<{
  id: string;
  /** The collection's path, e.g. `vh.inputs`. */
  path: string;
  labelField?: string;
}>();
const model = defineModel<string | boolean>({ required: true });

const { leaves } = useLiveState(() => props.path);
const choices = computed(() => collectionChoices(props.path, leaves.value, props.labelField));
</script>

<template>
  <select
    :id="id"
    v-model="model"
  >
    <option value="">
      {{ choices.length === 0 ? `Nothing in ${path} yet` : "Choose…" }}
    </option>
    <option
      v-for="choice in choices"
      :key="String(choice.key)"
      :value="String(choice.key)"
    >
      {{ choice.label }}
    </option>
  </select>
</template>
