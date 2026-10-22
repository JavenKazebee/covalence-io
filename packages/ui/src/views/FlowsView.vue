<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { api } from "../api";
import FlowEditor from "../components/FlowEditor.vue";
import { describeTrigger, formatDuration, runTone, type Flow } from "../flows/model";
import { flowStatuses } from "../flows/status";
import { useLiveState } from "../live/useLiveState";
import { hrefOf } from "../nav";
import { useCatalogStore } from "../stores/catalog";
import { useFlowsStore } from "../stores/flows";

defineProps<{ flow?: string }>();

const catalog = useCatalogStore();
const flows = useFlowsStore();
const { leaves } = useLiveState("flow");
const statuses = computed(() => flowStatuses(leaves.value));
const rows = computed(() => flows.flows.map((f) => ({ f, running: statuses.value.get(f.slug)?.running ?? 0, last: statuses.value.get(f.slug)?.lastRun ?? null })));

onMounted(() => void flows.refresh());

/** The last Run or toggle answer per flow, when it's worth saying. */
const notes = ref(new Map<string, { tone: string; text: string }>());

function note(id: string, tone: string, text: string) {
  notes.value = new Map(notes.value).set(id, { tone, text });
}

async function toggle(flow: Flow) {
  try {
    await flows.save({ ...flow, enabled: !flow.enabled });
    notes.value.delete(flow.id);
  } catch (e) {
    note(flow.id, "bad", e instanceof Error ? e.message : String(e));
  }
}

async function run(flow: Flow) {
  try {
    const fired = await api.runFlow(flow.id);
    if (fired.status === "blocked") note(flow.id, "warn", `Blocked: ${fired.reason}`);
    else if (fired.status === "queued") note(flow.id, "warn", "Queued");
    else notes.value.delete(flow.id);
  } catch (e) {
    note(flow.id, "bad", e instanceof Error ? e.message : String(e));
  }
}

const canRun = (flow: Flow) => flow.enabled && flow.triggers.some((t) => t.kind === "api");

function time(ms: number | undefined): string {
  return ms === undefined ? "" : new Date(ms).toLocaleTimeString(undefined, { hour12: false });
}
</script>

<template>
  <FlowEditor
    v-if="flow !== undefined"
    :id="flow"
    :status="statuses.get(flows.get(flow)?.slug ?? '')"
  />
  <section
    v-else
    class="panel"
  >
    <div class="row list-head">
      <h2>Flows</h2>
      <span class="spacer" />
      <a
        class="button"
        :href="hrefOf({ tab: 'flows', flow: 'new' })"
      >New flow</a>
    </div>
    <p
      v-if="flows.error"
      class="error"
    >
      {{ flows.error }}
    </p>
    <p
      v-else-if="!flows.loaded"
      class="muted"
    >
      Loading…
    </p>
    <p
      v-else-if="flows.flows.length === 0"
      class="muted"
    >
      No flows yet. A flow runs steps, like routing and waiting, when its trigger fires.
    </p>
    <table
      v-else
      class="flows"
    >
      <thead>
        <tr>
          <th>Enabled</th>
          <th>Name</th>
          <th>Triggers</th>
          <th>Running</th>
          <th>Last run</th>
          <th />
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="{ f, running, last } in rows"
          :key="f.id"
          :data-flow="f.slug"
        >
          <td>
            <input
              type="checkbox"
              :checked="f.enabled"
              :aria-label="`Enable ${f.name}`"
              @change="toggle(f)"
            >
          </td>
          <td>
            <a :href="hrefOf({ tab: 'flows', flow: f.id })">{{ f.name }}</a>
            <div class="muted">
              {{ f.body.length }} {{ f.body.length === 1 ? "step" : "steps" }} · {{ f.concurrency }}
            </div>
          </td>
          <td class="muted">
            {{ f.triggers.length === 0 ? "None" : f.triggers.map((t) => describeTrigger(t, catalog.alias)).join(", ") }}
          </td>
          <td>
            <span
              class="badge"
              :class="running > 0 ? 'warn' : 'muted'"
              data-testid="running"
            >{{ running }}</span>
          </td>
          <td data-testid="last-run">
            <template v-if="last?.status">
              <span
                class="badge"
                :class="runTone(last.status)"
              >{{ last.status }}</span>
              <span class="muted">
                {{ time(last.startedAt) }}
                <template v-if="last.durationMs !== undefined">
                  · {{ formatDuration(last.durationMs) }}
                </template>
              </span>
              <div
                v-if="last.error"
                class="error"
              >
                {{ last.error.message }}
              </div>
            </template>
            <span
              v-else
              class="muted"
            >Never</span>
          </td>
          <td>
            <button
              type="button"
              class="secondary"
              :disabled="!canRun(f)"
              :title="canRun(f) ? '' : f.enabled ? 'No API trigger' : 'Disabled'"
              :aria-label="`Run ${f.name}`"
              @click="run(f)"
            >
              Run
            </button>
            <div
              v-if="notes.get(f.id)"
              class="badge"
              :class="notes.get(f.id)?.tone"
            >
              {{ notes.get(f.id)?.text }}
            </div>
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>

<style scoped>
.list-head {
  margin-bottom: 0.75rem;
}

.list-head h2 {
  margin: 0;
}

.spacer {
  flex: 1;
}

a.button {
  padding: 0.4rem 0.9rem;
  border-radius: 6px;
  background: var(--accent);
  color: #fff;
  text-decoration: none;
}

.flows button.secondary {
  padding: 0.1rem 0.6rem;
}
</style>
