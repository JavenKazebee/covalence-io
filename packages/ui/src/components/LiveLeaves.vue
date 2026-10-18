<script setup lang="ts">
import { useLiveState } from "../live/useLiveState";

const props = defineProps<{ path: string }>();
const { leaves } = useLiveState(() => props.path);
</script>

<template>
  <table>
    <thead>
      <tr>
        <th>Path</th>
        <th>Value</th>
        <th>Quality</th>
        <th>Seq</th>
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="leaf in leaves"
        :key="leaf.path"
      >
        <td><code>{{ leaf.path }}</code></td>
        <td><code>{{ JSON.stringify(leaf.value) }}</code></td>
        <td>{{ leaf.quality }}</td>
        <td>{{ leaf.seq }}</td>
      </tr>
      <tr v-if="leaves.length === 0">
        <td colspan="4">
          Nothing under <code>{{ path }}</code> yet.
        </td>
      </tr>
    </tbody>
  </table>
</template>
