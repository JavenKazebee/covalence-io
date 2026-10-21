<script setup lang="ts">
import type { Quality } from "../live/protocol";

defineProps<{ quality: Quality }>();

const titles: Record<Quality, string> = {
  good: "Current",
  stale: "Source disconnected; showing the last value",
  unknown: "Never received",
};
</script>

<template>
  <span
    class="badge"
    :class="quality"
    :title="titles[quality]"
  >{{ quality }}</span>
</template>
