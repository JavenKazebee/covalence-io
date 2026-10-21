<script setup lang="ts">
import { computed } from "vue";
import { useLiveStore } from "../stores/live";

const live = useLiveStore();

const status = computed(() => {
  if (!live.connected) return { tone: "bad", text: "Server unreachable", detail: "Reconnecting…" };
  const host = live.host;
  if (host === null) return { tone: "muted", text: "Plugin host unknown", detail: "" };
  const detail = `generation ${host.generation}${host.pid === null ? "" : `, pid ${host.pid}`}`;
  switch (host.phase) {
    case "ready":
      return { tone: "good", text: "Plugin host ready", detail };
    case "starting":
      return { tone: "warn", text: "Plugin host starting", detail };
    case "down":
      return { tone: "bad", text: "Plugin host down", detail };
    case "stopped":
      return { tone: "muted", text: "Plugin host stopped", detail };
  }
  return { tone: "muted", text: host.phase, detail };
});
</script>

<template>
  <div
    class="host-status"
    :data-tone="status.tone"
    role="status"
    :title="status.detail"
  >
    <span class="dot" />
    <span>{{ status.text }}</span>
    <span
      v-if="live.lastError"
      class="error"
      :title="live.lastError"
    >· {{ live.lastError }}</span>
  </div>
</template>
