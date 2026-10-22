<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { api } from "../api";
import { useLiveClient } from "../live/useLiveState";
import { formatDuration, isActive, runTone, type Flow, type RunInfo } from "../flows/model";
import type { FlowStatus } from "../flows/status";
import { hrefOf } from "../nav";

/**
 * A saved flow's runs: Run and Cancel, and `GET /runs` refetched whenever the
 * journal tail shows this flow's `trigger.matched` or `run.*` entries, so the
 * current step stays live (flow spec §9).
 */
const props = defineProps<{ flow: Flow; status: FlowStatus | undefined }>();
const emit = defineEmits<{ position: [stepId: string | null] }>();

const runs = ref<RunInfo[]>([]);
const error = ref<string | null>(null);
const notice = ref<string | null>(null);
const busy = ref(false);

let fetching = 0;
async function refresh() {
  const mine = ++fetching;
  try {
    const list = await api.runs(props.flow.id);
    if (mine !== fetching) return;
    runs.value = list;
    error.value = null;
  } catch (e) {
    if (mine === fetching) error.value = e instanceof Error ? e.message : String(e);
  }
}

const client = useLiveClient();
let unfollow: (() => void) | null = null;
onMounted(() => {
  unfollow = client.followJournal({
    entry: (entry) => {
      const d = entry.data as { flow?: string } | null;
      const ours = d?.flow === props.flow.id || ("flow" in entry.origin && entry.origin.flow === props.flow.id);
      if (ours && (entry.kind === "trigger.matched" || entry.kind.startsWith("run."))) void refresh();
    },
    lagged: () => void refresh(),
  });
  void refresh();
});
onBeforeUnmount(() => unfollow?.());
watch(() => [props.flow.id, JSON.stringify(props.status ?? null)], refresh);
watch(
  runs,
  (list) => {
    const running = list.find((r) => r.status === "running");
    emit("position", running?.position?.[0] ?? null);
  },
);

const canRun = () => props.flow.enabled && props.flow.triggers.some((t) => t.kind === "api");

async function run() {
  busy.value = true;
  notice.value = null;
  try {
    const fired = await api.runFlow(props.flow.id);
    if (fired.status === "blocked") notice.value = `Blocked: ${fired.reason}.`;
    else if (fired.status === "queued") notice.value = "Queued behind the active run.";
  } catch (e) {
    notice.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
    await refresh();
  }
}

async function cancel(run: RunInfo) {
  try {
    await api.cancelRun(run.run);
  } catch (e) {
    notice.value = e instanceof Error ? e.message : String(e);
  } finally {
    await refresh();
  }
}

function time(ms: number | undefined): string {
  return ms === undefined ? "" : new Date(ms).toLocaleTimeString(undefined, { hour12: false });
}

const short = (id: string) => `…${id.slice(-6)}`;
</script>

<template>
  <section
    class="panel"
    aria-label="Runs"
  >
    <div class="row runs-head">
      <h3>Runs</h3>
      <span
        class="badge"
        :class="(status?.running ?? 0) > 0 ? 'warn' : 'muted'"
        data-testid="running"
      >{{ status?.running ?? 0 }} running</span>
      <span class="spacer" />
      <button
        type="button"
        :disabled="busy || !canRun()"
        :title="canRun() ? '' : flow.enabled ? 'Add an API trigger to run it from here' : 'The flow is disabled'"
        @click="run"
      >
        Run
      </button>
    </div>
    <p
      v-if="notice"
      class="muted"
      role="status"
    >
      {{ notice }}
    </p>
    <p
      v-if="error"
      class="error"
    >
      {{ error }}
    </p>
    <p
      v-if="runs.length === 0"
      class="muted"
    >
      No runs since the server started.
    </p>
    <table v-else>
      <thead>
        <tr>
          <th>Run</th>
          <th>Status</th>
          <th>Started</th>
          <th>Took</th>
          <th>Where</th>
          <th>Journal</th>
          <th />
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="r in runs"
          :key="r.run"
          :data-run="r.run"
          :data-status="r.status"
        >
          <td><code :title="r.run">{{ short(r.run) }}</code></td>
          <td>
            <span
              class="badge"
              :class="runTone(r.status)"
            >{{ r.status }}</span>
          </td>
          <td class="muted">
            {{ time(r.startedAt) }}
          </td>
          <td class="muted">
            {{ r.durationMs === undefined ? "" : formatDuration(r.durationMs) }}
          </td>
          <td>
            <code v-if="r.position">{{ r.position.join(" › ") }}</code>
            <span
              v-if="r.error"
              class="error"
            >{{ r.error.code }}: {{ r.error.message }}</span>
          </td>
          <td class="links">
            <a
              :href="hrefOf({ tab: 'journal', seq: r.matched })"
              title="trigger.matched"
            >#{{ r.matched }}</a>
            <a
              v-if="r.finished"
              :href="hrefOf({ tab: 'journal', seq: r.finished })"
              title="run.finished"
            >#{{ r.finished }}</a>
          </td>
          <td>
            <button
              v-if="isActive(r.status)"
              type="button"
              class="secondary"
              @click="cancel(r)"
            >
              Cancel
            </button>
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>

<style scoped>
.runs-head {
  margin-bottom: 0.5rem;
}

.runs-head h3 {
  margin: 0;
}

.spacer {
  flex: 1;
}

.links {
  display: flex;
  gap: 0.5rem;
}

button.secondary {
  padding: 0.1rem 0.6rem;
}
</style>
