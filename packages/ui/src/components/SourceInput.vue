<script setup lang="ts">
import { computed, shallowRef, watch } from "vue";
import { useAliases } from "../bindings/editing";
import { fullHex, isSimpleTemplate } from "../bindings/model";
import type { ShownProblem } from "../flows/model";
import {
  curves,
  fallbackOf,
  formNames,
  formOf,
  formats,
  literal,
  literalOf,
  literalText,
  newSource,
  ops,
  parseLiteral,
  toExpr,
  withFallback,
  type Curve,
  type Expect,
  type Form,
  type Json,
  type Ref,
  type Source,
  type Transform,
} from "../sources/model";
import ExprInput from "./ExprInput.vue";
import RefInput from "./bindings/RefInput.vue";
import TemplateInput from "./bindings/TemplateInput.vue";

/**
 * One source (dataflow §1, dataflow spec §10.1): a value, a path, an expression or a
 * transform, whose options are sources in turn. Binding properties and
 * action parameters both use it. A structured form shows the expression it
 * compiles to, and can become it.
 */
const props = withDefaults(
  defineProps<{
    label: string;
    /** What the source should give, so new forms start sensibly. */
    expect?: Expect;
    /** The forms offered; the defaults suit `expect`. */
    forms?: readonly Form[];
    /** Whether a fallback can be set here. */
    fallback?: boolean;
    /** The outermost source, which shows its compiled expression. */
    top?: boolean;
    /** The server's problems for this source and what's under it. */
    problems?: ShownProblem[];
    /** In a flow: the steps finished when this runs, and all of them, for `steps.<id>` warnings. */
    known?: ReadonlySet<string> | null;
    all?: ReadonlySet<string>;
  }>(),
  {
    expect: "any",
    forms: undefined,
    fallback: false,
    top: false,
    problems: () => [],
    known: null,
    all: () => new Set<string>(),
  },
);
const model = defineModel<Source>({ required: true });

const aliases = useAliases();

const defaultForms: Record<Expect, readonly Form[]> = {
  text: ["literal", "ref", "format", "template", "rules", "map", "expr"],
  color: ["literal", "ref", "rules", "map", "expr"],
  boolean: ["compare", "all", "any", "not", "ref", "expr", "literal"],
  number: ["literal", "ref", "rules", "map", "expr"],
  any: ["literal", "ref", "expr", "format", "template", "rules", "map", "compare", "all", "any", "not"],
};

const form = computed(() => formOf(model.value));
const offered = computed(() => {
  const list = props.forms ?? defaultForms[props.expect];
  return list.includes(form.value) ? list : [...list, form.value];
});

/** Each form's last shape, so trying another and coming back loses nothing. */
const stash = new Map<Form, Source>();

function setForm(next: Form) {
  stash.set(form.value, model.value);
  model.value = stash.get(next) ?? newSource(next, props.expect);
  converted.value = null;
}

// ---- Typed views of the current transform ----

type Of<U extends Transform["use"]> = Extract<Transform, { use: U }> & { fallback?: Json };
const view = <U extends Transform["use"]>() => model.value as unknown as Of<U>;
const fmt = computed(() => view<"format">());
const tpl = computed(() => view<"template">());
const rules = computed(() => view<"rules">());
const cmp = computed(() => view<"compare">());
const list = computed(() => view<"all" | "any">());
const not = computed(() => view<"not">());
const map = computed(() => view<"map">());
const refSource = computed(() => model.value as { ref: Ref; fallback?: Json });
const exprSource = computed(() => model.value as { expr: string; fallback?: Json });

// ---- Literals ----

const literalValue = computed(() => literalOf(model.value));

function setLiteral(value: Json | undefined) {
  model.value = literal(value === undefined ? "" : value);
}

function setNumber(target: number[], i: number, text: string) {
  const n = Number(text);
  if (text.trim() !== "" && Number.isFinite(n)) target[i] = n;
}

const isRange = (to: unknown[]) => to.length === 2 && to.every((v) => typeof v === "number");

function setMapMode(range: boolean) {
  map.value.to = range ? [0, 100] : props.expect === "color" ? ["#000000", "#ffffff"] : ["Low", "High"];
}

function setClamp(on: boolean) {
  if (on) delete map.value.clamp;
  else map.value.clamp = false;
}

function setCompareValue(text: string) {
  const v = parseLiteral(text);
  if (v === undefined) delete cmp.value.value;
  else cmp.value.value = literal(v);
}

const compareValueText = computed(() => {
  const v = cmp.value.value;
  return v === undefined ? "" : formOf(v) === "literal" ? literalText(literalOf(v)) : "";
});

// ---- Fallback ----

const fallbackValue = computed(() => fallbackOf(model.value));

function setFallback(text: string) {
  model.value = withFallback(model.value, parseLiteral(text));
}

// ---- Converting to an expression (one-way, unless it's left as it was) ----

const compiled = computed(() => (form.value === "literal" || form.value === "expr" ? null : toExpr(model.value, aliases.value)));
const converted = shallowRef<{ from: Source; expr: string } | null>(null);

function convert() {
  if (compiled.value === null) return;
  const from = model.value;
  const fallback = fallbackOf(from);
  model.value = fallback === undefined ? { expr: compiled.value } : { expr: compiled.value, fallback };
  converted.value = { from, expr: compiled.value };
}

const canGoBack = computed(
  () => converted.value !== null && form.value === "expr" && exprSource.value.expr === converted.value.expr,
);

function back() {
  if (converted.value === null) return;
  model.value = converted.value.from;
  converted.value = null;
}

// Something else replaced the source (another key, a reload): forget its history.
watch(
  () => props.label,
  () => {
    converted.value = null;
    stash.clear();
  },
);

const exprProblems = computed(() => props.problems.filter((p) => p.path[p.path.length - 1] === "expr"));
const literalType = computed(() => (props.expect === "color" ? "color" : "text"));
</script>

<template>
  <span
    class="source-input"
    :class="{ top, nested: !top }"
    :data-form="form"
  >
    <span class="head">
      <select
        :value="form"
        :aria-label="`${label} form`"
        class="form"
        @change="setForm(($event.target as HTMLSelectElement).value as Form)"
      >
        <option
          v-for="f in offered"
          :key="f"
          :value="f"
        >
          {{ formNames[f] }}
        </option>
      </select>

      <!-- A value: typed by what's expected. -->
      <template v-if="form === 'literal'">
        <select
          v-if="expect === 'boolean'"
          :value="String(literalValue)"
          :aria-label="label"
          @change="setLiteral(($event.target as HTMLSelectElement).value === 'true')"
        >
          <option value="true">true</option>
          <option value="false">false</option>
        </select>
        <span
          v-else-if="literalType === 'color'"
          class="color"
        >
          <input
            type="color"
            class="swatch"
            :value="fullHex(String(literalValue ?? '')) ?? '#000000'"
            :aria-label="`${label} picker`"
            @input="setLiteral(($event.target as HTMLInputElement).value)"
          >
          <input
            :value="literalValue ?? ''"
            :aria-label="label"
            class="mono short"
            placeholder="#rrggbb"
            spellcheck="false"
            @input="setLiteral(($event.target as HTMLInputElement).value)"
          >
        </span>
        <input
          v-else-if="expect === 'text'"
          :value="literalValue === null ? '' : String(literalValue)"
          :aria-label="label"
          spellcheck="false"
          @input="setLiteral(($event.target as HTMLInputElement).value)"
        >
        <input
          v-else
          :value="literalText(literalValue)"
          :aria-label="label"
          class="mono"
          title="A number, true, false, null, JSON or text. Quote text that looks like a number."
          spellcheck="false"
          @change="setLiteral(parseLiteral(($event.target as HTMLInputElement).value))"
        >
      </template>

      <RefInput
        v-else-if="form === 'ref'"
        v-model="refSource.ref"
        :label="`${label} path`"
      />

      <template v-else-if="form === 'format'">
        <SourceInput
          v-model="fmt.input"
          :label="`${label} input`"
          :forms="['ref', 'expr']"
          :problems="problems"
        />
        <select
          v-model="fmt.format"
          :aria-label="`${label} format`"
        >
          <option
            v-for="f in formats"
            :key="f"
            :value="f"
          >
            {{ f }}
          </option>
        </select>
      </template>

      <template v-else-if="form === 'compare'">
        <SourceInput
          v-model="cmp.input"
          :label="`${label} input`"
          :forms="['ref', 'expr', 'format', 'map']"
          :problems="problems"
        />
        <select
          v-model="cmp.op"
          :aria-label="`${label} comparison`"
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
          v-if="cmp.op !== 'truthy' && cmp.op !== 'falsy'"
          class="mono short"
          :value="compareValueText"
          :aria-label="`${label} value`"
          title="A number, true, false, null or text. Quote text that looks like a number."
          spellcheck="false"
          @change="setCompareValue(($event.target as HTMLInputElement).value)"
        >
      </template>

      <template v-else-if="form === 'not'">
        <SourceInput
          v-model="not.input"
          :label="`${label} input`"
          expect="boolean"
          :problems="problems"
        />
      </template>
      <button
        v-if="!top && compiled !== null && form !== 'ref'"
        type="button"
        class="secondary icon"
        :aria-label="`Edit ${label} as an expression`"
        :title="`As an expression: ${compiled}`"
        @click="convert"
      >
        { }
      </button>
    </span>

    <ExprInput
      v-if="form === 'expr'"
      v-model="exprSource.expr"
      :label="`${label} expression`"
      :problems="exprProblems"
      :known="known"
      :all="all"
    />

    <TemplateInput
      v-else-if="form === 'template' && isSimpleTemplate(tpl.parts)"
      v-model="tpl.parts"
      :label="`${label} template`"
    />
    <ol
      v-else-if="form === 'template'"
      class="items"
    >
      <li
        v-for="(_, i) in tpl.parts"
        :key="i"
      >
        <SourceInput
          v-model="tpl.parts[i]!"
          :label="`${label} part ${i + 1}`"
          expect="text"
          :problems="problems"
        />
      </li>
    </ol>

    <template v-else-if="form === 'rules'">
      <ol class="items">
        <li
          v-for="(rule, i) in rules.rules"
          :key="i"
          class="rule"
        >
          <span class="muted">When</span>
          <SourceInput
            v-model="rule.when"
            :label="`${label} rule ${i + 1} condition`"
            expect="boolean"
            :problems="problems"
          />
          <span class="muted">→</span>
          <SourceInput
            v-model="rule.value"
            :label="`${label} rule ${i + 1} result`"
            :expect="expect"
            :forms="['literal', 'ref', 'expr']"
            :problems="problems"
          />
          <button
            type="button"
            class="secondary icon"
            :aria-label="`Remove ${label} rule ${i + 1}`"
            title="Remove"
            @click="rules.rules.splice(i, 1)"
          >
            ×
          </button>
        </li>
      </ol>
      <span class="row">
        <button
          type="button"
          class="secondary"
          @click="rules.rules.push({ when: newSource('compare'), value: newSource('literal', expect) })"
        >
          Add rule
        </button>
        <span class="muted">Otherwise</span>
        <SourceInput
          v-model="rules.else"
          :label="`${label} otherwise`"
          :expect="expect"
          :forms="['literal', 'ref', 'expr']"
          :problems="problems"
        />
      </span>
    </template>

    <template v-else-if="form === 'all' || form === 'any'">
      <ol class="items">
        <li
          v-for="(_, i) in list.of"
          :key="i"
          class="rule"
        >
          <SourceInput
            v-model="list.of[i]!"
            :label="`${label} item ${i + 1}`"
            expect="boolean"
            :problems="problems"
          />
          <button
            type="button"
            class="secondary icon"
            :aria-label="`Remove ${label} item ${i + 1}`"
            title="Remove"
            @click="list.of.splice(i, 1)"
          >
            ×
          </button>
        </li>
      </ol>
      <button
        type="button"
        class="secondary"
        @click="list.of.push(newSource('compare'))"
      >
        Add condition
      </button>
    </template>

    <template v-else-if="form === 'map'">
      <span class="row">
        <SourceInput
          v-model="map.input"
          :label="`${label} input`"
          :forms="['ref', 'expr']"
          :problems="problems"
        />
        <span class="muted">from</span>
        <input
          type="number"
          class="num"
          :value="map.from[0]"
          :aria-label="`${label} from low`"
          step="any"
          @change="setNumber(map.from, 0, ($event.target as HTMLInputElement).value)"
        >
        <span class="muted">to</span>
        <input
          type="number"
          class="num"
          :value="map.from[1]"
          :aria-label="`${label} from high`"
          step="any"
          @change="setNumber(map.from, 1, ($event.target as HTMLInputElement).value)"
        >
      </span>
      <span class="row">
        <span class="muted">onto</span>
        <select
          v-if="expect !== 'color'"
          :value="isRange(map.to) ? 'range' : 'list'"
          :aria-label="`${label} map onto`"
          @change="setMapMode(($event.target as HTMLSelectElement).value === 'range')"
        >
          <option value="range">a range</option>
          <option value="list">a list</option>
        </select>
        <template v-if="expect !== 'color' && isRange(map.to)">
          <input
            type="number"
            class="num"
            :value="map.to[0]"
            :aria-label="`${label} onto low`"
            step="any"
            @change="setNumber(map.to as number[], 0, ($event.target as HTMLInputElement).value)"
          >
          <span class="muted">to</span>
          <input
            type="number"
            class="num"
            :value="map.to[1]"
            :aria-label="`${label} onto high`"
            step="any"
            @change="setNumber(map.to as number[], 1, ($event.target as HTMLInputElement).value)"
          >
        </template>
        <template v-else>
          <span
            v-for="(item, i) in map.to"
            :key="i"
            class="list-item"
          >
            <input
              :value="literalText(item as Json)"
              :aria-label="`${label} list item ${i + 1}`"
              class="mono short"
              spellcheck="false"
              @change="(map.to as Json[])[i] = parseLiteral(($event.target as HTMLInputElement).value) ?? ''"
            >
            <button
              type="button"
              class="secondary icon"
              :aria-label="`Remove ${label} list item ${i + 1}`"
              title="Remove"
              :disabled="map.to.length <= 1"
              @click="(map.to as Json[]).splice(i, 1)"
            >
              ×
            </button>
          </span>
          <button
            type="button"
            class="secondary"
            @click="(map.to as Json[]).push(expect === 'color' ? '#ffffff' : '')"
          >
            Add
          </button>
        </template>
        <select
          :value="map.curve ?? 'linear'"
          :aria-label="`${label} curve`"
          @change="map.curve = ($event.target as HTMLSelectElement).value as Curve"
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
          :checked="map.clamp !== false"
          @change="setClamp(($event.target as HTMLInputElement).checked)"
        > Clamp</label>
      </span>
    </template>

    <span
      v-if="fallback && form !== 'literal'"
      class="row"
    >
      <span class="muted">If it's undefined or fails</span>
      <input
        :value="literalText(fallbackValue)"
        :aria-label="`${label} fallback`"
        class="mono short"
        :placeholder="expect === 'color' ? 'the default' : 'nothing'"
        spellcheck="false"
        @change="setFallback(($event.target as HTMLInputElement).value)"
      >
    </span>

    <span
      v-if="compiled !== null && top"
      class="compiled row"
    >
      <span class="muted">As an expression</span>
      <code data-testid="compiled">{{ compiled }}</code>
      <button
        type="button"
        class="secondary"
        @click="convert"
      >
        Edit as expression
      </button>
    </span>
    <span
      v-if="canGoBack"
      class="row"
    >
      <button
        type="button"
        class="secondary"
        @click="back"
      >
        Back to {{ formNames[formOf(converted!.from)].toLowerCase() }}
      </button>
      <span class="hint">Only until the expression is edited.</span>
    </span>
  </span>
</template>

<style scoped>
.source-input {
  display: inline-grid;
  gap: 0.35rem;
  min-width: 0;
}

.source-input.top {
  display: grid;
}

.source-input.nested {
  border-left: 2px solid var(--border);
  padding-left: 0.4rem;
}

.head,
.row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.4rem;
}

.items {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: 0.35rem;
}

.rule {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.4rem;
}

.mono {
  font-family: var(--mono);
  font-size: 0.92em;
}

.short {
  width: 7rem;
}

.num {
  width: 5.5rem;
}

.color {
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

.list-item {
  display: inline-flex;
  gap: 0.2rem;
  align-items: center;
}

button.icon {
  padding: 0.15rem 0.55rem;
}

.compiled code {
  background: var(--code-bg);
  padding: 0.1rem 0.4rem;
  border-radius: 4px;
  word-break: break-all;
}
</style>
