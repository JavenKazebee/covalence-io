<script setup lang="ts">
import { computed } from "vue";
import type { JsonSchema } from "../api";
import { problemsAt, type ParamDef, type ShownProblem } from "../flows/model";
import { formOf, literal, literalOf, literalText, parseLiteral, type Source } from "../sources/model";

/**
 * A flow's inputs and outputs (flow spec §8.1): each a name and a type; an
 * input also has a default and whether it must be given. Other schema
 * fields (`minimum`, `kind` …) are kept as they are.
 */
const props = defineProps<{ problems: ShownProblem[] }>();
const inputs = defineModel<Record<string, ParamDef>>("inputs", { required: true });
const outputs = defineModel<Record<string, JsonSchema>>("outputs", { required: true });

type Type = NonNullable<JsonSchema["type"]> | "";
const types: { id: Type; name: string }[] = [
  { id: "", name: "Any" },
  { id: "string", name: "Text" },
  { id: "number", name: "Number" },
  { id: "integer", name: "Whole number" },
  { id: "boolean", name: "Yes or no" },
];

type Which = "inputs" | "outputs";
const listOf = (which: Which): Record<string, ParamDef | JsonSchema> => (which === "inputs" ? inputs.value : outputs.value);
function setList(which: Which, next: Record<string, ParamDef | JsonSchema>) {
  if (which === "inputs") inputs.value = next as Record<string, ParamDef>;
  else outputs.value = next as Record<string, JsonSchema>;
}

/** Renames an entry in place, keeping the order. */
function rename(which: Which, from: string, to: string) {
  const name = to.trim();
  if (name === "" || name === from || name in listOf(which)) return;
  setList(which, Object.fromEntries(Object.entries(listOf(which)).map(([k, v]) => [k === from ? name : k, v])));
}

function add(which: Which) {
  const list = listOf(which);
  let n = Object.keys(list).length + 1;
  while (`${which === "inputs" ? "input" : "output"}${n}` in list) n++;
  setList(which, { ...list, [`${which === "inputs" ? "input" : "output"}${n}`]: { type: "string" } });
}

function remove(which: Which, name: string) {
  const next = { ...listOf(which) };
  delete next[name];
  setList(which, next);
}

function setType(def: ParamDef | JsonSchema, type: Type) {
  if (type === "") delete def.type;
  else def.type = type;
}

/** A literal default as text; one that follows state can't be edited here. */
const defaultText = (def: ParamDef) => (def.default === undefined ? "" : literalText(literalOf(def.default)));
const defaultIsLiteral = (def: ParamDef) => def.default === undefined || formOf(def.default) === "literal";
function setDefault(def: ParamDef, text: string) {
  const value = parseLiteral(text);
  if (value === undefined) delete def.default;
  else def.default = literal(value) as Source;
}

function setRequired(def: ParamDef, on: boolean) {
  if (on) def.required = true;
  else delete def.required;
}

const at = (which: Which, name: string) => problemsAt(props.problems, [which, name]);
const inputNames = computed(() => Object.keys(inputs.value));
const outputNames = computed(() => Object.keys(outputs.value));
</script>

<template>
  <div class="io">
    <div>
      <h4>Inputs</h4>
      <p class="muted">
        Given each time it starts: by a <code>call</code> step or the API. Steps read them as <code>input.&lt;name&gt;</code>.
      </p>
      <ul>
        <li
          v-for="name in inputNames"
          :key="name"
          class="row"
        >
          <input
            :value="name"
            :aria-label="`Input ${name} name`"
            class="mono name"
            spellcheck="false"
            @change="rename('inputs', name, ($event.target as HTMLInputElement).value)"
          >
          <select
            :value="inputs[name]!.type ?? ''"
            :aria-label="`Input ${name} type`"
            @change="setType(inputs[name]!, ($event.target as HTMLSelectElement).value as Type)"
          >
            <option
              v-for="t in types"
              :key="t.id"
              :value="t.id"
            >
              {{ t.name }}
            </option>
          </select>
          <input
            v-if="defaultIsLiteral(inputs[name]!)"
            :value="defaultText(inputs[name]!)"
            :aria-label="`Input ${name} default`"
            class="mono"
            placeholder="No default"
            spellcheck="false"
            @change="setDefault(inputs[name]!, ($event.target as HTMLInputElement).value)"
          >
          <span
            v-else
            class="muted"
          >Default follows state</span>
          <label class="check">
            <input
              type="checkbox"
              :checked="inputs[name]!.required === true"
              @change="setRequired(inputs[name]!, ($event.target as HTMLInputElement).checked)"
            >
            Required
          </label>
          <button
            type="button"
            class="secondary icon"
            :aria-label="`Remove input ${name}`"
            title="Remove"
            @click="remove('inputs', name)"
          >
            ×
          </button>
          <ul
            v-if="at('inputs', name).length > 0"
            class="problems"
          >
            <li
              v-for="(p, j) in at('inputs', name)"
              :key="j"
              :class="p.warning ? 'warn' : 'error'"
            >
              {{ p.message }}
            </li>
          </ul>
        </li>
      </ul>
      <button
        type="button"
        class="secondary"
        @click="add('inputs')"
      >
        Add input
      </button>
    </div>
    <div>
      <h4>Outputs</h4>
      <p class="muted">
        Handed back to a caller by a <code>stop</code> step.
      </p>
      <ul>
        <li
          v-for="name in outputNames"
          :key="name"
          class="row"
        >
          <input
            :value="name"
            :aria-label="`Output ${name} name`"
            class="mono name"
            spellcheck="false"
            @change="rename('outputs', name, ($event.target as HTMLInputElement).value)"
          >
          <select
            :value="outputs[name]!.type ?? ''"
            :aria-label="`Output ${name} type`"
            @change="setType(outputs[name]!, ($event.target as HTMLSelectElement).value as Type)"
          >
            <option
              v-for="t in types"
              :key="t.id"
              :value="t.id"
            >
              {{ t.name }}
            </option>
          </select>
          <button
            type="button"
            class="secondary icon"
            :aria-label="`Remove output ${name}`"
            title="Remove"
            @click="remove('outputs', name)"
          >
            ×
          </button>
          <ul
            v-if="at('outputs', name).length > 0"
            class="problems"
          >
            <li
              v-for="(p, j) in at('outputs', name)"
              :key="j"
              :class="p.warning ? 'warn' : 'error'"
            >
              {{ p.message }}
            </li>
          </ul>
        </li>
      </ul>
      <button
        type="button"
        class="secondary"
        @click="add('outputs')"
      >
        Add output
      </button>
    </div>
  </div>
</template>

<style scoped>
.io {
  display: grid;
  gap: 16px;
}
ul {
  list-style: none;
  margin: 0 0 8px;
  padding: 0;
  display: grid;
  gap: 6px;
}
.row {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}
.name {
  width: 10em;
}
.check {
  display: inline-flex;
  gap: 4px;
  align-items: center;
}
.problems {
  flex-basis: 100%;
}
h4 {
  margin: 0 0 4px;
}
</style>
