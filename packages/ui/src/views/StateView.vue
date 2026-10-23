<script setup lang="ts">
import { computed, ref, shallowReactive, watch } from "vue";
import QualityBadge from "../components/QualityBadge.vue";
import { useLiveState } from "../live/useLiveState";
import { hrefOf, navigate } from "../nav";
import { buildTree, visibleRows } from "../state/tree";
import { useCatalogStore } from "../stores/catalog";
import { useLiveStore } from "../stores/live";
import { useSurfacesStore } from "../stores/surfaces";

const props = defineProps<{ path?: string }>();
const catalog = useCatalogStore();
const live = useLiveStore();
const surfaces = useSurfacesStore();

/** With no path in the URL, show the first connection, or else the first surface's controls. */
const root = computed(() => {
  const surface = surfaces.surfaces[0];
  return props.path ?? catalog.connections[0]?.alias ?? (surface === undefined ? "" : `surface.${surface.slug}.controls`);
});
const draft = ref(root.value);
watch(root, (next) => (draft.value = next));

const { leaves } = useLiveState(root);
const tree = computed(() => buildTree(root.value, leaves.value));
const collapsed = shallowReactive(new Set<string>());
watch(root, () => collapsed.clear());
const rows = computed(() => visibleRows(tree.value, collapsed));

function toggle(path: string) {
  if (collapsed.has(path)) collapsed.delete(path);
  else collapsed.add(path);
}

function go() {
  const path = draft.value.trim();
  if (path !== "") navigate({ tab: "state", path });
}

function show(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

function time(ts: number): string {
  return new Date(ts).toLocaleTimeString(undefined, { hour12: false });
}
</script>

<template>
  <section class="panel">
    <form
      class="row"
      @submit.prevent="go"
    >
      <h2 style="margin: 0">
        State
      </h2>
      <label
        for="state-path"
        class="muted"
      >Path</label>
      <input
        id="state-path"
        v-model="draft"
        spellcheck="false"
        autocomplete="off"
        placeholder="vh.outputs"
        style="flex: 1; max-width: 24rem"
      >
      <button
        type="submit"
        class="secondary"
      >
        Show
      </button>
      <span class="muted">
        <a
          v-for="conn in catalog.connections"
          :key="conn.id"
          :href="hrefOf({ tab: 'state', path: conn.alias })"
          style="margin-right: 0.5rem"
        ><code>{{ conn.alias }}</code></a>
        <!-- Each surface's control input: pressed, detents, value, touched (surfaces spec §3). -->
        <a
          v-for="s in surfaces.surfaces"
          :key="s.id"
          :href="hrefOf({ tab: 'state', path: `surface.${s.slug}.controls` })"
          :title="`${s.name}: its controls' input`"
          style="margin-right: 0.5rem"
        ><code>surface.{{ s.slug }}.controls</code></a>
      </span>
    </form>
  </section>

  <section class="panel">
    <p
      v-if="root === ''"
      class="muted"
    >
      No connections yet. Create one under <a :href="hrefOf({ tab: 'connections' })">Connections</a>, or look at a surface's
      control input under <code>surface</code>.
    </p>
    <p
      v-else-if="rows.length === 0"
      class="muted"
    >
      Nothing under <code>{{ root }}</code> yet.
    </p>
    <table
      v-else
      class="state-tree"
      :class="{ offline: !live.connected }"
    >
      <thead>
        <tr>
          <th>Path</th>
          <th>Value</th>
          <th>Quality</th>
          <th title="Last changed, and the journal entry that changed it">
            Changed
          </th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="{ node, depth } in rows"
          :key="node.path"
          :data-path="node.path"
        >
          <td :style="{ paddingLeft: `${0.5 + depth * 1.1}rem` }">
            <button
              v-if="node.children.length > 0"
              class="twisty"
              :aria-expanded="!collapsed.has(node.path)"
              :aria-label="`${collapsed.has(node.path) ? 'Expand' : 'Collapse'} ${node.path}`"
              @click="toggle(node.path)"
            >
              {{ collapsed.has(node.path) ? "▸" : "▾" }}
            </button>
            <span
              v-else
              class="twisty-space"
            />
            <code :title="node.path">{{ node.label }}</code>
            <span
              v-if="node.children.length > 0 && collapsed.has(node.path)"
              class="muted"
            > ({{ node.count }})</span>
          </td>
          <td
            class="value"
            :class="{ 'stale-value': node.quality === 'stale' }"
          >
            <code v-if="node.leaf">{{ show(node.leaf.value) }}</code>
          </td>
          <td>
            <QualityBadge
              v-if="node.leaf || node.quality !== 'good'"
              :quality="node.quality"
            />
          </td>
          <td>
            <template v-if="node.leaf">
              <span class="muted">{{ time(node.leaf.ts) }}</span>
              <a
                :href="hrefOf({ tab: 'journal', seq: node.leaf.seq })"
                :title="`Why: journal entry ${node.leaf.seq}`"
              > #{{ node.leaf.seq }}</a>
            </template>
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>

<style scoped>
.twisty {
  border: none;
  background: none;
  color: var(--muted);
  padding: 0;
  width: 1.1rem;
  text-align: left;
}

.twisty-space {
  display: inline-block;
  width: 1.1rem;
}

.offline {
  opacity: 0.6;
}

.value {
  word-break: break-word;
}
</style>
