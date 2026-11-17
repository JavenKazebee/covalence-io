<script setup lang="ts">
import { computed } from "vue";
import { propNames, propType, type Binding, type Preview, type Prop } from "../../bindings/model";
import { pathText, type ShownProblem } from "../../flows/model";
import { formNames, newSource, type Form } from "../../sources/model";
import SourceInput from "../SourceInput.vue";

/**
 * One property of a key (bindings spec §2): unbound, or a source, and what
 * the preview makes of it.
 */
const props = defineProps<{
  prop: Prop;
  /** The preview's view of this property, when there is one. */
  shown?: Preview["props"][Prop];
  /** Save problems under `props.<prop>`. */
  problems: ShownProblem[];
  /** The error this property gave when the core last evaluated it. */
  runtime?: string;
}>();
const model = defineModel<Binding | undefined>({ required: true });

const type = computed(() => propType(props.prop));
const name = computed(() => propNames[props.prop]);

/** The forms a property is bound with first: M5's five choices, and a path. */
const firstForms: readonly Form[] = ["literal", "ref", "template", "rules", "map", "expr"];
const offered = computed(() => firstForms.filter((f) => f !== "template" || type.value === "text"));

let last: Binding | undefined;

function bind(form: Form | "") {
  if (form === "") {
    last = model.value;
    model.value = undefined;
  } else {
    model.value = last ?? newSource(form, type.value);
    last = undefined;
  }
}

/** Problems the source's own fields don't show: all but expressions', with where they are. */
const otherProblems = computed(() =>
  props.problems
    .filter((p) => p.path[p.path.length - 1] !== "expr")
    .map((p) => ({ ...p, where: p.path.length > 2 ? pathText(p.path.slice(2)) : "" })),
);
</script>

<template>
  <fieldset
    class="prop"
    :data-prop="prop"
  >
    <legend>
      <span class="prop-name">{{ name }}</span>
      <select
        v-if="model === undefined"
        value=""
        :aria-label="`${name} binding`"
        @change="bind(($event.target as HTMLSelectElement).value as Form | '')"
      >
        <option value="">
          Not bound
        </option>
        <option
          v-for="f in offered"
          :key="f"
          :value="f"
        >
          {{ formNames[f] }}
        </option>
      </select>
      <button
        v-else
        type="button"
        class="secondary"
        :aria-label="`Unbind ${name}`"
        @click="bind('')"
      >
        Unbind
      </button>
    </legend>

    <p
      v-if="model === undefined"
      class="muted"
    >
      The key's default: {{ prop === "label" ? "no label" : prop === "background" ? "black" : "white" }}.
    </p>
    <SourceInput
      v-else
      v-model="model"
      :label="name"
      :expect="type"
      :problems="problems"
      fallback
      top
    />

    <div
      v-if="model !== undefined"
      class="outcome"
    >
      <div
        v-if="shown?.value !== undefined || shown?.error"
        class="now"
        data-testid="prop-preview"
      >
        <span
          v-if="type === 'color' && shown?.value"
          class="chip"
          :style="{ background: shown.value }"
        />
        <code v-if="shown?.value !== undefined">= {{ JSON.stringify(shown.value) }}</code>
        <span
          v-if="shown?.error"
          class="warn"
        >{{ shown.value !== undefined ? "(fallback) " : "" }}{{ shown.error }}</span>
        <span
          v-if="shown?.reads.length"
          class="muted"
        > · reads {{ shown.reads.join(", ") }}</span>
      </div>
      <small
        v-if="runtime"
        class="error"
      >Last evaluated: {{ runtime }}</small>
      <small
        v-for="(p, i) in otherProblems"
        :key="i"
        :class="p.warning ? 'warn' : 'error'"
      >{{ p.where ? `${p.where}: ` : "" }}{{ p.message }}</small>
    </div>
  </fieldset>
</template>

<style scoped>
.prop {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 0.5rem 0.9rem 0.8rem;
  margin: 0 0 0.75rem;
  display: grid;
  gap: 0.5rem;
  min-width: 0;
}

legend {
  display: flex;
  gap: 0.6rem;
  align-items: center;
  padding: 0 0.3rem;
}

.prop-name {
  font-weight: 600;
}

.rules {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: 0.4rem;
}

.rules .row,
.prop .row {
  gap: 0.5rem;
}

.literal {
  width: 6rem;
  font-family: var(--mono);
  font-size: 0.92em;
}

.num {
  width: 5.5rem;
}

.list-item {
  display: inline-flex;
  gap: 0.2rem;
  align-items: center;
}

button.icon {
  padding: 0.15rem 0.55rem;
}

.outcome {
  display: grid;
  gap: 0.25rem;
}

.now {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.4rem;
}

.chip {
  display: inline-block;
  width: 1rem;
  height: 1rem;
  border-radius: 4px;
  border: 1px solid var(--border);
}

.compiled code {
  background: var(--code-bg);
  padding: 0.1rem 0.4rem;
  border-radius: 4px;
  word-break: break-all;
}

.warn {
  color: var(--warn);
}
</style>
