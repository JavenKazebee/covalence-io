<script setup lang="ts">
import { computed, ref } from "vue";
import WhyDrawer from "../components/WhyDrawer.vue";
import { describe, describeOrigin, toneOf, type SummaryContext } from "../journal/summary";
import { KEEP, useJournal } from "../journal/useJournal";
import { hrefOf, navigate } from "../nav";
import { useCatalogStore } from "../stores/catalog";

defineProps<{ seq?: number }>();
const catalog = useCatalogStore();
const { entries, error, loading } = useJournal();

const filter = ref("");
const hidePatches = ref(false);

/** Call ID → `vh.route`, from the requests on screen. */
const calls = computed(() => {
  const map = new Map<string, string>();
  for (const entry of entries.value) {
    if (entry.kind !== "action.requested") continue;
    const d = entry.data as { call: string; conn: string; action: string };
    map.set(d.call, `${catalog.alias(d.conn)}.${d.action}`);
  }
  return map;
});

const ctx: SummaryContext = {
  alias: (id) => catalog.alias(id),
  callName: (call) => calls.value.get(call),
};

const rows = computed(() => {
  const needle = filter.value.trim().toLowerCase();
  return entries.value
    .filter((e) => !(hidePatches.value && e.kind === "state.patch"))
    .map((entry) => ({ entry, text: describe(entry, ctx), origin: describeOrigin(entry.origin, ctx) }))
    .filter((r) => needle === "" || `${r.entry.kind} ${r.text} ${r.origin}`.toLowerCase().includes(needle));
});

function time(ts: string): string {
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleTimeString(undefined, { hour12: false });
}
</script>

<template>
  <section class="panel">
    <div
      class="row"
      style="margin-bottom: 0.75rem"
    >
      <h2 style="margin: 0">
        Journal
      </h2>
      <input
        v-model="filter"
        type="search"
        placeholder="Filter"
        aria-label="Filter entries"
        style="flex: 1; max-width: 20rem"
      >
      <label class="muted"><input
        v-model="hidePatches"
        type="checkbox"
      > Hide state patches</label>
      <span class="muted">Newest first · last {{ KEEP }} kept · click an entry for its why chain</span>
    </div>
    <p
      v-if="error"
      class="error"
    >
      {{ error }}
    </p>
    <p
      v-else-if="loading && rows.length === 0"
      class="muted"
    >
      Loading…
    </p>
    <table
      v-else
      class="journal"
    >
      <thead>
        <tr>
          <th>#</th>
          <th>Time</th>
          <th>Kind</th>
          <th>Origin</th>
          <th>What</th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="{ entry, text, origin } in rows"
          :key="entry.seq"
          :class="{ selected: entry.seq === seq }"
          :data-seq="entry.seq"
          :data-kind="entry.kind"
          tabindex="0"
          @click="navigate({ tab: 'journal', seq: entry.seq })"
          @keydown.enter="navigate({ tab: 'journal', seq: entry.seq })"
        >
          <td>
            <a
              :href="hrefOf({ tab: 'journal', seq: entry.seq })"
              @click.stop
            >{{ entry.seq }}</a>
          </td>
          <td class="muted">
            {{ time(entry.ts) }}
          </td>
          <td>
            <span
              class="badge"
              :class="toneOf(entry)"
            >{{ entry.kind }}</span>
          </td>
          <td class="muted">
            {{ origin }}
          </td>
          <td>
            {{ text }}
            <span
              v-if="entry.cause"
              class="muted"
            > ← #{{ entry.cause }}</span>
          </td>
        </tr>
        <tr v-if="rows.length === 0">
          <td
            colspan="5"
            class="muted"
          >
            No entries.
          </td>
        </tr>
      </tbody>
    </table>
  </section>
  <WhyDrawer
    v-if="seq !== undefined"
    :seq="seq"
    :ctx="ctx"
  />
</template>

<style scoped>
.journal tbody tr {
  cursor: pointer;
}

.journal tbody tr:hover,
.journal tbody tr.selected {
  background: var(--code-bg);
}

.badge.muted {
  color: var(--muted);
}
</style>
