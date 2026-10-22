<script setup lang="ts">
import { onMounted, watch } from "vue";
import HostStatus from "./components/HostStatus.vue";
import { hrefOf, tabs, useRoute, type Tab } from "./nav";
import { useCatalogStore } from "./stores/catalog";
import { useFlowsStore } from "./stores/flows";
import { useLiveStore } from "./stores/live";
import ActionsView from "./views/ActionsView.vue";
import ConnectionsView from "./views/ConnectionsView.vue";
import FlowsView from "./views/FlowsView.vue";
import JournalView from "./views/JournalView.vue";
import StateView from "./views/StateView.vue";
import { useLiveClient } from "./live/useLiveState";

const route = useRoute();
const live = useLiveStore();
const catalog = useCatalogStore();
const flows = useFlowsStore();

const titles: Record<Tab, string> = {
  connections: "Connections",
  state: "State",
  actions: "Actions",
  flows: "Flows",
  journal: "Journal",
};

// The header shows host status from the start, before any screen subscribes.
useLiveClient();
// Flow names are for the journal too, not just the Flows tab.
onMounted(() => void Promise.all([catalog.refresh(), flows.refresh()]));
// Manifests appear once the host has loaded the plugins.
watch(
  () => live.host?.phase === "ready" && live.host.generation,
  (ready) => {
    if (ready !== false) void catalog.refresh();
  },
);
</script>

<template>
  <header class="app-header">
    <h1>Covalence IO</h1>
    <nav aria-label="Screens">
      <a
        v-for="tab in tabs"
        :key="tab"
        :href="hrefOf({ tab })"
        :aria-current="route.tab === tab ? 'page' : undefined"
      >{{ titles[tab] }}</a>
    </nav>
    <HostStatus />
  </header>
  <main>
    <ConnectionsView v-if="route.tab === 'connections'" />
    <StateView
      v-else-if="route.tab === 'state'"
      :path="route.path"
    />
    <ActionsView v-else-if="route.tab === 'actions'" />
    <FlowsView
      v-else-if="route.tab === 'flows'"
      :flow="route.flow"
    />
    <JournalView
      v-else
      :seq="route.seq"
    />
  </main>
</template>
