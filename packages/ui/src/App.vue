<script setup lang="ts">
import { ref } from "vue";
import LiveLeaves from "./components/LiveLeaves.vue";
import { useLiveStore } from "./stores/live";

const live = useLiveStore();
const path = ref("vh.outputs");
</script>

<template>
  <header>
    <h1>Covalence IO</h1>
    <p>
      Socket: {{ live.connected ? "open" : "closed" }} ·
      Plugin host: {{ live.host ? `${live.host.phase} (generation ${live.host.generation})` : "unknown" }}
    </p>
    <p v-if="live.lastError">
      Error: {{ live.lastError }}
    </p>
  </header>
  <main>
    <label>
      Path <input
        v-model.lazy="path"
        spellcheck="false"
      >
    </label>
    <LiveLeaves :path="path" />
  </main>
</template>
