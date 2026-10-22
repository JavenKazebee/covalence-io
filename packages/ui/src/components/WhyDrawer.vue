<script setup lang="ts">
import { ref, watch } from "vue";
import { api } from "../api";
import { describe, describeOrigin, type SummaryContext } from "../journal/summary";
import type { JournalEntry } from "../live/protocol";
import { hrefOf } from "../nav";

/**
 * "Why did this happen?" for one journal entry: the entry, then each `cause`
 * back to the root (state spec §2.2).
 */
const props = defineProps<{ seq: number; ctx: SummaryContext }>();

const chain = ref<JournalEntry[]>([]);
const error = ref<string | null>(null);
const loading = ref(false);

watch(
  () => props.seq,
  async (seq) => {
    loading.value = true;
    error.value = null;
    try {
      const entries = await api.why(seq);
      if (seq === props.seq) chain.value = entries;
    } catch (e) {
      if (seq !== props.seq) return;
      chain.value = [];
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      if (seq === props.seq) loading.value = false;
    }
  },
  { immediate: true },
);

/** The flow an entry belongs to: from its origin, or `trigger.matched` and `run.started` data. */
function flowOf(entry: JournalEntry): string | undefined {
  if ("flow" in entry.origin) return entry.origin.flow;
  const flow = (entry.data as { flow?: unknown } | null)?.flow;
  return typeof flow === "string" ? flow : undefined;
}

function time(ts: string): string {
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleTimeString(undefined, { hour12: false });
}
</script>

<template>
  <aside
    class="why-drawer"
    aria-label="Why"
  >
    <header class="row">
      <h3 style="margin: 0">
        Why #{{ seq }}
      </h3>
      <a
        :href="hrefOf({ tab: 'journal' })"
        class="close"
        aria-label="Close"
      >×</a>
    </header>
    <p
      v-if="error"
      class="error"
    >
      {{ error }}
    </p>
    <p
      v-else-if="loading && chain.length === 0"
      class="muted"
    >
      Loading…
    </p>
    <template v-else-if="chain[0]">
      <p class="muted">
        {{ chain.length === 1 ? "Nothing caused this entry: it is a root." : `Caused by ${chain.length - 1} earlier ${chain.length === 2 ? "entry" : "entries"}, newest first.` }}
      </p>
      <ol class="chain">
        <li
          v-for="(entry, i) in chain"
          :key="entry.seq"
          :class="{ selected: i === 0 }"
          :data-seq="entry.seq"
        >
          <div class="row">
            <a :href="hrefOf({ tab: 'journal', seq: entry.seq })">#{{ entry.seq }}</a>
            <code>{{ entry.kind }}</code>
            <span class="muted">{{ time(entry.ts) }} · {{ describeOrigin(entry.origin, ctx) }}</span>
            <a
              v-if="flowOf(entry)"
              :href="hrefOf({ tab: 'flows', flow: flowOf(entry) })"
            >Open flow</a>
          </div>
          <div>{{ describe(entry, ctx) }}</div>
          <details :open="i === 0">
            <summary class="muted">
              Data
            </summary>
            <pre>{{ JSON.stringify(entry.data, null, 2) }}</pre>
          </details>
        </li>
      </ol>
    </template>
  </aside>
</template>

<style scoped>
.why-drawer {
  position: fixed;
  top: 0;
  right: 0;
  bottom: 0;
  width: min(30rem, 100vw);
  overflow-y: auto;
  background: var(--panel);
  border-left: 1px solid var(--border);
  box-shadow: -8px 0 24px rgb(0 0 0 / 12%);
  padding: 1rem;
  z-index: 10;
}

.why-drawer header {
  justify-content: space-between;
  margin-bottom: 0.5rem;
}

.close {
  font-size: 1.4rem;
  text-decoration: none;
  color: var(--muted);
}

.chain {
  list-style: none;
  padding: 0;
  margin: 0;
  display: grid;
  gap: 0.75rem;
}

.chain li {
  border-left: 3px solid var(--border);
  padding-left: 0.6rem;
}

.chain li.selected {
  border-left-color: var(--accent);
}

pre {
  background: var(--code-bg);
  padding: 0.5rem;
  border-radius: 6px;
  overflow-x: auto;
  margin: 0.25rem 0 0;
}
</style>
