<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  curves,
  formats,
  kindNames,
  kinds,
  literalText,
  newBinding,
  ops,
  parseLiteral,
  propNames,
  propType,
  toExpr,
  type Binding,
  type BindingKind,
  type Preview,
  type Prop,
  type Value,
} from "../../bindings/model";
import type { ShownProblem } from "../../flows/model";
import ExprInput from "../ExprInput.vue";
import RefInput from "./RefInput.vue";
import TemplateInput from "./TemplateInput.vue";
import ValueField from "./ValueField.vue";

/**
 * One property of a key (bindings spec §2): which kind of binding, its
 * fields, and what the preview makes of it. The structured kinds show the
 * expression they compile to, and can become it.
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

/** Each kind's last form, so trying another kind and coming back loses nothing. */
const stash = new Map<BindingKind, Binding>();

function setKind(kind: BindingKind | "") {
  if (model.value !== undefined) stash.set(model.value.kind, model.value);
  model.value = kind === "" ? undefined : (stash.get(kind) ?? newBinding(kind, props.prop));
  converted.value = null;
}

// ---- Converting to `expr` (one-way, unless it's left as it was) ----

const converted = ref<{ from: Binding; source: string } | null>(null);
const canConvert = computed(() => model.value !== undefined && model.value.kind !== "expr" && props.shown?.expr !== undefined);

function convert() {
  if (model.value === undefined || props.shown?.expr === undefined) return;
  const from = model.value;
  model.value = toExpr(from, props.shown.expr);
  converted.value = { from, source: props.shown.expr };
}

const canGoBack = computed(
  () => converted.value !== null && model.value?.kind === "expr" && model.value.source === converted.value.source,
);

function back() {
  if (converted.value === null) return;
  model.value = converted.value.from;
  converted.value = null;
}

// Another key, or a reload: whatever was converted belongs to the old binding.
watch(
  () => props.prop,
  () => {
    converted.value = null;
    stash.clear();
  },
);

// ---- Fields ----

const at = (field: string) => props.problems.filter((p) => p.path[2] === field);
const otherProblems = computed(() => props.problems.filter((p) => p.path.length <= 2 || !["source"].includes(String(p.path[2]))));

function setFallback(value: Value | undefined) {
  const b = model.value;
  if (b === undefined || b.kind === "rules" || b.kind === "map") return;
  if (value === undefined || value === "") delete b.fallback;
  else b.fallback = value as string;
}

function setFormat(format: string) {
  const b = model.value;
  if (b?.kind !== "value") return;
  if (format === "") delete b.format;
  else b.format = format as (typeof formats)[number];
}

function setNumber(target: number[], i: number, text: string) {
  const n = Number(text);
  if (text.trim() !== "" && Number.isFinite(n)) target[i] = n;
}

function setConditionValue(rule: { when: { value?: Value } }, text: string) {
  const value = parseLiteral(text);
  if (value === undefined) delete rule.when.value;
  else rule.when.value = value;
}

/** A map's `to`: two numbers make a range, anything else is a list to pick from. */
const isRange = (to: unknown[]) => to.length === 2 && to.every((v) => typeof v === "number");

function setMapMode(range: boolean) {
  const b = model.value;
  if (b?.kind !== "map") return;
  b.to = range ? [0, 100] : type.value === "color" ? ["#000000", "#ffffff"] : ["Low", "High"];
}

function addRule() {
  const b = model.value;
  if (b?.kind !== "rules") return;
  b.rules.push({ when: { path: { root: "vars", path: [] }, op: "==", value: 1 }, value: type.value === "color" ? "#e02020" : "" });
}

function setClamp(on: boolean) {
  const b = model.value;
  if (b?.kind !== "map") return;
  if (on) delete b.clamp;
  else b.clamp = false;
}
</script>

<template>
  <fieldset
    class="prop"
    :data-prop="prop"
  >
    <legend>
      <span class="prop-name">{{ name }}</span>
      <select
        :value="model?.kind ?? ''"
        :aria-label="`${name} binding kind`"
        @change="setKind(($event.target as HTMLSelectElement).value as BindingKind | '')"
      >
        <option value="">
          Not bound
        </option>
        <option
          v-for="k in kinds"
          :key="k"
          :value="k"
        >
          {{ kindNames[k] }}
        </option>
      </select>
    </legend>

    <template v-if="model === undefined">
      <p class="muted">
        The key's default: {{ prop === "label" ? "no label" : prop === "background" ? "black" : "white" }}.
      </p>
    </template>

    <template v-else-if="model.kind === 'value'">
      <div class="row">
        <RefInput
          v-model="model.path"
          :label="`${name} path`"
        />
        <select
          :value="model.format ?? ''"
          :aria-label="`${name} format`"
          @change="setFormat(($event.target as HTMLSelectElement).value)"
        >
          <option value="">
            As it is
          </option>
          <option
            v-for="f in formats.filter((f) => f !== 'auto')"
            :key="f"
            :value="f"
          >
            {{ f }}
          </option>
          <option
            v-if="model.format === 'auto'"
            value="auto"
          >
            auto
          </option>
        </select>
      </div>
    </template>

    <template v-else-if="model.kind === 'template'">
      <div class="row">
        <TemplateInput
          v-model="model.parts"
          :label="`${name} template`"
        />
      </div>
    </template>

    <template v-else-if="model.kind === 'rules'">
      <p class="muted">
        The first rule that's true wins.
      </p>
      <ol class="rules">
        <li
          v-for="(rule, i) in model.rules"
          :key="i"
          class="row"
        >
          <span class="muted">When</span>
          <RefInput
            v-model="rule.when.path"
            :label="`${name} rule ${i + 1} path`"
          />
          <select
            v-model="rule.when.op"
            :aria-label="`${name} rule ${i + 1} comparison`"
          >
            <option
              v-for="o in ops"
              :key="o"
              :value="o"
            >
              {{ o === "truthy" ? "is truthy" : o === "falsy" ? "is falsy" : o }}
            </option>
          </select>
          <input
            v-if="rule.when.op !== 'truthy' && rule.when.op !== 'falsy'"
            class="literal"
            :value="literalText(rule.when.value)"
            :aria-label="`${name} rule ${i + 1} value`"
            title="A number, true, false, null or text. Quote text that looks like a number."
            spellcheck="false"
            @change="setConditionValue(rule, ($event.target as HTMLInputElement).value)"
          >
          <span class="muted">→</span>
          <ValueField
            v-model="rule.value"
            :type="type"
            :label="`${name} rule ${i + 1} result`"
          />
          <button
            type="button"
            class="secondary icon"
            :aria-label="`Remove ${name} rule ${i + 1}`"
            title="Remove"
            @click="model.rules.splice(i, 1)"
          >
            ×
          </button>
        </li>
      </ol>
      <div class="row">
        <button
          type="button"
          class="secondary"
          @click="addRule"
        >
          Add rule
        </button>
        <span class="muted">Otherwise</span>
        <ValueField
          v-model="model.else"
          :type="type"
          :label="`${name} otherwise`"
        />
      </div>
    </template>

    <template v-else-if="model.kind === 'map'">
      <div class="row">
        <RefInput
          v-model="model.path"
          :label="`${name} path`"
        />
        <span class="muted">from</span>
        <input
          type="number"
          class="num"
          :value="model.from[0]"
          :aria-label="`${name} from low`"
          step="any"
          @change="setNumber(model.from, 0, ($event.target as HTMLInputElement).value)"
        >
        <span class="muted">to</span>
        <input
          type="number"
          class="num"
          :value="model.from[1]"
          :aria-label="`${name} from high`"
          step="any"
          @change="setNumber(model.from, 1, ($event.target as HTMLInputElement).value)"
        >
      </div>
      <div class="row">
        <span class="muted">onto</span>
        <select
          v-if="type === 'text'"
          :value="isRange(model.to) ? 'range' : 'list'"
          :aria-label="`${name} map onto`"
          @change="setMapMode(($event.target as HTMLSelectElement).value === 'range')"
        >
          <option value="range">
            a range
          </option>
          <option value="list">
            a list
          </option>
        </select>
        <template v-if="type === 'text' && isRange(model.to)">
          <input
            type="number"
            class="num"
            :value="model.to[0]"
            :aria-label="`${name} onto low`"
            step="any"
            @change="setNumber(model.to as number[], 0, ($event.target as HTMLInputElement).value)"
          >
          <span class="muted">to</span>
          <input
            type="number"
            class="num"
            :value="model.to[1]"
            :aria-label="`${name} onto high`"
            step="any"
            @change="setNumber(model.to as number[], 1, ($event.target as HTMLInputElement).value)"
          >
        </template>
        <template v-else>
          <span
            v-for="(_, i) in model.to"
            :key="i"
            class="list-item"
          >
            <ValueField
              v-model="model.to[i]"
              :type="type"
              :label="`${name} list item ${i + 1}`"
            />
            <button
              type="button"
              class="secondary icon"
              :aria-label="`Remove ${name} list item ${i + 1}`"
              title="Remove"
              :disabled="model.to.length <= 1"
              @click="(model.to as unknown[]).splice(i, 1)"
            >
              ×
            </button>
          </span>
          <button
            type="button"
            class="secondary"
            @click="(model.to as unknown[]).push(type === 'color' ? '#ffffff' : '')"
          >
            Add
          </button>
        </template>
      </div>
      <div class="row">
        <select
          :value="model.curve ?? 'linear'"
          :aria-label="`${name} curve`"
          @change="model.curve = ($event.target as HTMLSelectElement).value as (typeof curves)[number]"
        >
          <option
            v-for="c in curves"
            :key="c"
            :value="c"
          >
            {{ c }}
          </option>
        </select>
        <label><input
          type="checkbox"
          :checked="model.clamp !== false"
          @change="setClamp(($event.target as HTMLInputElement).checked)"
        > Clamp to the range</label>
      </div>
    </template>

    <template v-else-if="model.kind === 'expr'">
      <ExprInput
        v-model="model.source"
        :label="`${name} expression`"
        :placeholder="type === 'color' ? 'vh.outputs[1].source == 3 ? &quot;#e02020&quot; : &quot;#333&quot;' : '`Out 1: ${vh.outputs[1].source}`'"
        :problems="at('source')"
      />
      <p
        v-if="canGoBack"
        class="row"
      >
        <button
          type="button"
          class="secondary"
          @click="back"
        >
          Back to {{ kindNames[converted!.from.kind].toLowerCase() }}
        </button>
        <span class="hint">Only until the expression is edited.</span>
      </p>
    </template>

    <div
      v-if="model !== undefined && model.kind !== 'rules' && model.kind !== 'map'"
      class="row"
    >
      <span class="muted">If it's undefined or fails, show</span>
      <ValueField
        :model-value="model.fallback"
        :type="type"
        :label="`${name} fallback`"
        :placeholder="type === 'color' ? 'the default' : 'nothing'"
        optional
        @update:model-value="setFallback"
      />
    </div>

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
      <div
        v-if="model.kind !== 'expr' && shown?.expr"
        class="compiled row"
      >
        <span class="muted">As an expression</span>
        <code data-testid="compiled">{{ shown.expr }}</code>
        <button
          type="button"
          class="secondary"
          :disabled="!canConvert"
          @click="convert"
        >
          Edit as expression
        </button>
      </div>
      <small
        v-if="runtime"
        class="error"
      >Last evaluated: {{ runtime }}</small>
      <small
        v-for="(p, i) in otherProblems"
        :key="i"
        :class="p.warning ? 'warn' : 'error'"
      >{{ p.message }}</small>
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
