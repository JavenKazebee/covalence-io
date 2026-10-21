<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { api, type ActionOutcome } from "../api";
import SchemaForm from "../components/SchemaForm.vue";
import { fieldsOf, initialValues, parseValues } from "../forms/schema";
import { hrefOf } from "../nav";
import { useCatalogStore } from "../stores/catalog";

const catalog = useCatalogStore();

const connId = ref("");
const actionId = ref("");
const values = ref<Record<string, string | boolean>>({});
const errors = ref<Record<string, string>>({});
const busy = ref(false);

interface Run {
  /** `vh.route`. */
  action: string;
  params: Record<string, unknown>;
  outcome?: ActionOutcome;
  failure?: string;
}
/** This page's runs, newest first. */
const runs = ref<Run[]>([]);

const conn = computed(() => catalog.connections.find((c) => c.id === connId.value) ?? null);
const manifest = computed(() => (conn.value === null ? null : catalog.manifest(conn.value.plugin)));
const actions = computed(() => manifest.value?.actions ?? []);
const action = computed(() => actions.value.find((a) => a.id === actionId.value) ?? null);
const fields = computed(() => (action.value === null ? [] : fieldsOf(action.value.params)));

watch(
  () => catalog.connections,
  (conns) => {
    if (!conns.some((c) => c.id === connId.value)) connId.value = conns[0]?.id ?? "";
  },
  { immediate: true },
);
watch(
  actions,
  (list) => {
    if (!list.some((a) => a.id === actionId.value)) actionId.value = list[0]?.id ?? "";
  },
  { immediate: true },
);
watch(fields, (next) => {
  values.value = initialValues(next);
  errors.value = {};
}, { immediate: true });

async function run() {
  if (conn.value === null || action.value === null) return;
  const parsed = parseValues(fields.value, values.value);
  errors.value = parsed.ok ? {} : parsed.errors;
  if (!parsed.ok) return;

  const entry: Run = { action: `${conn.value.alias}.${action.value.id}`, params: parsed.value };
  busy.value = true;
  try {
    entry.outcome = await api.requestAction(entry.action, entry.params);
  } catch (e) {
    entry.failure = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
    runs.value = [entry, ...runs.value].slice(0, 10);
  }
}

function summary(run: Run): { tone: string; text: string } {
  const o = run.outcome;
  if (o === undefined) return { tone: "bad", text: run.failure ?? "Failed" };
  if (typeof o.verdict === "object") return { tone: "warn", text: `Suppressed: ${o.verdict.suppressed}` };
  if (o.ok === true) return { tone: "good", text: "OK" };
  if (o.ok === false) return { tone: "bad", text: o.error ? `${o.error.code}: ${o.error.message}` : "Failed" };
  return { tone: "warn", text: "Dispatched, no result yet" };
}
</script>

<template>
  <section class="panel">
    <h2>Run an action</h2>
    <p
      v-if="catalog.connections.length === 0"
      class="muted"
    >
      No connections yet. Create one under <a :href="hrefOf({ tab: 'connections' })">Connections</a>.
    </p>
    <form
      v-else
      novalidate
      @submit.prevent="run"
    >
      <div class="row">
        <div class="field">
          <label for="action-conn">Connection</label>
          <select
            id="action-conn"
            v-model="connId"
          >
            <option
              v-for="c in catalog.connections"
              :key="c.id"
              :value="c.id"
            >
              {{ c.alias }}
            </option>
          </select>
        </div>
        <div class="field">
          <label for="action-id">Action</label>
          <select
            id="action-id"
            v-model="actionId"
            :disabled="actions.length === 0"
          >
            <option
              v-for="a in actions"
              :key="a.id"
              :value="a.id"
            >
              {{ a.name }} ({{ a.id }})
            </option>
          </select>
        </div>
      </div>
      <p
        v-if="manifest === null"
        class="muted"
      >
        Waiting for the plugin host to load this connection's plugin…
      </p>
      <template v-else-if="action !== null">
        <p
          v-if="action.description"
          class="muted"
        >
          {{ action.description }}
        </p>
        <SchemaForm
          v-model="values"
          :fields="fields"
          :errors="errors"
          :alias="conn?.alias"
          :manifest="manifest"
          id-prefix="action-param"
        />
        <button
          type="submit"
          :disabled="busy"
        >
          {{ busy ? "Running…" : `Run ${conn?.alias}.${action.id}` }}
        </button>
      </template>
    </form>
  </section>

  <section
    v-if="runs.length > 0"
    class="panel"
  >
    <h3>Results</h3>
    <table>
      <thead>
        <tr>
          <th>Action</th>
          <th>Params</th>
          <th>Outcome</th>
          <th>Journal</th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="(r, i) in runs"
          :key="r.outcome?.call ?? `failed-${i}`"
          class="action-run"
        >
          <td><code>{{ r.action }}</code></td>
          <td><code>{{ JSON.stringify(r.params) }}</code></td>
          <td>
            <span
              class="badge"
              :class="summary(r).tone"
            >{{ summary(r).text }}</span>
            <code
              v-if="r.outcome?.result != null"
              class="muted"
            > {{ JSON.stringify(r.outcome.result) }}</code>
          </td>
          <td>
            <a
              v-if="r.outcome"
              :href="hrefOf({ tab: 'journal', seq: r.outcome.resultSeq ?? r.outcome.requested })"
            >#{{ r.outcome.resultSeq ?? r.outcome.requested }}</a>
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>
