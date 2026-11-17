<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { api, ApiError, type TrafficEntry, type Traffic } from "../api";
import { asText, clockTime, sizeOf, spacedHex } from "../devices/model";
import { hrefOf } from "../nav";
import { useCatalogStore } from "../stores/catalog";

/**
 * A connection's traffic (ADR 0025): what went in and out of each handle the
 * core opened for it, newest first. The core keeps the last 500 entries of
 * each and the first 512 bytes of each message. Looks again every second
 * unless paused.
 */
const props = defineProps<{ conn: string }>();

const POLL_MS = 1000;

const catalog = useCatalogStore();
const connection = computed(() => catalog.connections.find((c) => c.alias === props.conn || c.id === props.conn));
const traffic = ref<Traffic | null>(null);
const error = ref<string | null>(null);
const missing = ref(false);
const paused = ref(false);
const hexOnly = ref(false);
let timer: ReturnType<typeof setTimeout> | undefined;
let stopped = false;

async function load() {
  clearTimeout(timer);
  const asked = props.conn;
  try {
    const next = await api.traffic(asked);
    if (asked !== props.conn) return;
    traffic.value = next;
    error.value = null;
    missing.value = false;
  } catch (e) {
    if (asked !== props.conn) return;
    missing.value = e instanceof ApiError && e.status === 404;
    error.value = e instanceof Error ? e.message : String(e);
  }
  if (!stopped && !paused.value && !missing.value) timer = setTimeout(load, POLL_MS);
}

watch(
  () => props.conn,
  () => {
    traffic.value = null;
    void load();
  },
  { immediate: true },
);
watch(paused, (p) => (p ? clearTimeout(timer) : void load()));
onBeforeUnmount(() => {
  stopped = true;
  clearTimeout(timer);
});

const arrows: Record<TrafficEntry["dir"], string> = { in: "←", out: "→", state: "•" };
const dirTitles: Record<TrafficEntry["dir"], string> = {
  in: "From the device or peer",
  out: "To the device or peer",
  state: "The handle opened, closed or failed",
};

function content(entry: TrafficEntry): { text: string; mono: boolean } {
  if (entry.hex === undefined) return { text: entry.note ?? "", mono: false };
  const text = hexOnly.value ? null : asText(entry.hex);
  return { text: text ?? spacedHex(entry.hex), mono: true };
}

const newestFirst = (entries: TrafficEntry[]) => [...entries].reverse();
</script>

<template>
  <p>
    <a :href="hrefOf({ tab: 'connections' })">← All connections</a>
  </p>
  <section class="panel traffic">
    <h2>
      Traffic of <code>{{ connection?.alias ?? conn }}</code>
    </h2>
    <p
      v-if="missing"
      class="muted"
    >
      No such connection.
    </p>
    <template v-else>
      <p
        v-if="error"
        class="error"
      >
        {{ error }}
      </p>
      <p
        v-if="traffic === null"
        class="muted"
      >
        Loading…
      </p>
      <p
        v-else-if="!traffic.inspectable"
        class="not-inspectable"
        role="note"
      >
        <span class="badge">not inspectable</span>
        Its plugin declares <code>directNetwork</code>: it opens its own sockets, so Covalence doesn't see what
        it sends.
      </p>
      <template v-if="traffic !== null && traffic.inspectable">
        <div class="row controls">
          <label><input
            v-model="paused"
            type="checkbox"
          > Pause</label>
          <label><input
            v-model="hexOnly"
            type="checkbox"
          > Hex only</label>
        </div>
        <p
          v-if="traffic.handles.length === 0"
          class="muted"
        >
          It hasn't opened anything through the core yet.
        </p>
        <section
          v-for="h in traffic.handles"
          :key="h.handle"
          class="handle"
          :data-handle="h.handle"
        >
          <h3>
            <span class="muted">{{ h.kind.toUpperCase() }}</span>
            <code>{{ h.target }}</code>
            <span
              class="badge"
              :class="h.open ? 'good' : ''"
            >{{ h.open ? "open" : "closed" }}</span>
          </h3>
          <p
            v-if="h.entries.length === 0"
            class="muted"
          >
            Nothing yet.
          </p>
          <table v-else>
            <thead>
              <tr>
                <th>Time</th>
                <th>
                  <span class="visually-hidden">Direction</span>
                </th>
                <th>Bytes</th>
                <th>Data</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="(e, i) in newestFirst(h.entries)"
                :key="`${e.ts}-${h.entries.length - i}`"
                :data-dir="e.dir"
              >
                <td class="muted time">
                  {{ clockTime(e.ts) }}
                </td>
                <td
                  class="dir"
                  :class="e.dir"
                  :title="dirTitles[e.dir]"
                >
                  {{ arrows[e.dir] }}
                </td>
                <td class="muted bytes">
                  <template v-if="sizeOf(e)">
                    {{ sizeOf(e)?.len }}<span
                      v-if="sizeOf(e)?.cut"
                      title="Only the first 512 bytes are kept"
                    >, cut</span>
                  </template>
                </td>
                <td>
                  <code
                    v-if="content(e).mono"
                    class="data"
                  >{{ content(e).text }}</code>
                  <span
                    v-else
                    class="muted"
                  >{{ content(e).text }}</span>
                  <span
                    v-if="e.hex !== undefined && e.note"
                    class="muted"
                  > {{ e.note }}</span>
                </td>
              </tr>
            </tbody>
          </table>
        </section>
      </template>
    </template>
  </section>
</template>

<style scoped>
.controls {
  gap: 1rem;
  margin-bottom: 0.5rem;
}

.handle h3 {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 1rem;
}

.handle table {
  font-size: 0.9em;
}

.time,
.bytes {
  white-space: nowrap;
}

.dir {
  text-align: center;
}

.dir.in {
  color: var(--good);
}

.dir.out {
  color: var(--accent);
}

.data {
  white-space: pre-wrap;
  word-break: break-all;
}
</style>
