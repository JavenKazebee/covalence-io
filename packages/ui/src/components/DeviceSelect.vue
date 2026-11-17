<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { api, type DeviceEntry } from "../api";

/**
 * Picks a device for a connection's `format: "device"` field: the devices
 * the plugin's `devices` match, and that no connection holds (plugin
 * protocol §7.1).
 */
const props = defineProps<{
  id: string;
  plugin: string;
}>();
const model = defineModel<string | boolean>({ required: true });

const devices = ref<DeviceEntry[]>([]);
const available = ref(true);
const error = ref<string | null>(null);
const loading = ref(false);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const list = await api.devices(props.plugin);
    available.value = list.available;
    devices.value = list.devices;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(load);
watch(() => props.plugin, load);

/** Free devices, and the chosen one even if it's held or unplugged. */
const choices = computed(() => {
  const free = devices.value.filter((d) => d.heldBy === undefined || d.id === model.value);
  const chosen = typeof model.value === "string" && model.value !== "" ? model.value : null;
  const missing = chosen !== null && !free.some((d) => d.id === chosen);
  return { free, missing: missing ? chosen : null };
});

function label(d: DeviceEntry): string {
  const name = d.product ?? d.manufacturer ?? d.kind.toUpperCase();
  return d.serial ? `${name} (${d.serial})` : `${name} at ${d.path}`;
}
</script>

<template>
  <div class="row">
    <select
      :id="id"
      v-model="model"
    >
      <option value="">
        {{
          !available
            ? "This server doesn't look for devices"
            : choices.free.length === 0
              ? "No matching devices plugged in"
              : "Choose…"
        }}
      </option>
      <option
        v-for="d in choices.free"
        :key="d.id"
        :value="d.id"
      >
        {{ label(d) }}
      </option>
      <option
        v-if="choices.missing"
        :value="choices.missing"
      >
        {{ choices.missing }} (not plugged in)
      </option>
    </select>
    <button
      type="button"
      class="secondary"
      :disabled="loading"
      title="Look again"
      @click="load"
    >
      ↻
    </button>
  </div>
  <small
    v-if="error"
    class="error"
  >{{ error }}</small>
</template>
