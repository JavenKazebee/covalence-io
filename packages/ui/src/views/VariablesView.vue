<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from "vue";
import { api } from "../api";
import VariableEditor from "../components/VariableEditor.vue";
import { useLiveState } from "../live/useLiveState";
import { hrefOf } from "../nav";
import { useCatalogStore } from "../stores/catalog";
import { describeSink, kindNames, kindOf, type CellProblem, type Variable } from "../variables/model";

/**
 * Declared variables (dataflow spec §6): what each is, its live value at
 * `vars.<slug>`, and where computed ones send their value.
 */
defineProps<{ variable?: string }>();

const catalog = useCatalogStore();
// Replaced whole on each load; shallow, as their sources nest deeply.
const variables = shallowRef<Variable[]>([]);
const problems = ref<CellProblem[]>([]);
const loaded = ref(false);
const error = ref<string | null>(null);

async function refresh() {
  try {
    const [v, p] = await Promise.all([api.variables(), api.cellProblems()]);
    variables.value = v.sort((a, b) => a.name.localeCompare(b.name));
    problems.value = p;
    error.value = null;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loaded.value = true;
  }
}

onMounted(() => void Promise.all([refresh(), catalog.refresh()]));
// A cell's problems come and go with state.
const timer = setInterval(async () => {
  try {
    problems.value = await api.cellProblems();
  } catch {
    // Kept as they were; the next try may work.
  }
}, 5000);
onBeforeUnmount(() => clearInterval(timer));

const { leaves } = useLiveState("vars");
const valueOf = (slug: string) => {
  const leaf = leaves.value.find((l) => l.path === `vars.${slug}`);
  return leaf === undefined ? "—" : JSON.stringify(leaf.value);
};
const problemOf = (id: string) => problems.value.find((p) => p.cell === `variable:${id}`);
const rows = computed(() => variables.value.map((v) => ({ v, kind: kindOf(v), problem: problemOf(v.id) })));
</script>

<template>
  <VariableEditor
    v-if="variable !== undefined"
    :id="variable"
    :variables="variables"
    :problem="problemOf(variable)"
    @saved="refresh"
  />
  <section
    v-else
    class="panel"
  >
    <div class="row list-head">
      <h2>Variables</h2>
      <span class="spacer" />
      <a
        class="button"
        :href="hrefOf({ tab: 'variables', variable: 'new' })"
      >New variable</a>
    </div>
    <p
      v-if="error"
      class="error"
    >
      {{ error }}
    </p>
    <p
      v-else-if="!loaded"
      class="muted"
    >
      Loading…
    </p>
    <p
      v-else-if="variables.length === 0"
      class="muted"
    >
      No variables declared yet. A variable is a value at <code>vars.&lt;name&gt;</code>: set by flows, or computed and
      kept up to date from other state.
    </p>
    <table
      v-else
      class="variables"
    >
      <thead>
        <tr>
          <th>Name</th>
          <th>Path</th>
          <th>Kind</th>
          <th>Value</th>
          <th>Sends to</th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="{ v, kind, problem } in rows"
          :key="v.id"
          :data-variable="v.slug"
        >
          <td>
            <a :href="hrefOf({ tab: 'variables', variable: v.id })">{{ v.name }}</a>
            <span
              v-if="v.persist"
              class="muted"
              title="Kept across a restart"
            > · kept</span>
          </td>
          <td><code>vars.{{ v.slug }}</code></td>
          <td>{{ kindNames[kind].split(":")[0] }}</td>
          <td>
            <code>{{ valueOf(v.slug) }}</code>
            <span
              v-if="problem"
              class="error"
              :title="problem.message"
            > {{ problem.code }}</span>
          </td>
          <td class="muted">
            {{ (v.to ?? []).map((s) => describeSink(s, catalog.alias)).join("; ") }}
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>

<style scoped>
.list-head h2 {
  margin: 0;
}

.variables {
  width: 100%;
  border-collapse: collapse;
}

.variables th,
.variables td {
  text-align: left;
  padding: 0.35rem 0.5rem;
  border-bottom: 1px solid var(--border, #2a2b30);
}
</style>
