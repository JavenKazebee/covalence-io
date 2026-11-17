<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { api, type DeviceEntry, type DeviceList } from "../api";
import { deviceLabel, hex16 } from "../devices/model";
import { hrefOf } from "../nav";
import { useCatalogStore } from "../stores/catalog";

/**
 * What's plugged in (plugin protocol §7.1): which connection holds each
 * device, the state of its handle, and offers for devices no connection
 * holds. Hotplug isn't on the WebSocket, so this looks again every 2 s.
 */
const emit = defineEmits<{ setUp: [plugin: string, device: string] }>();

const POLL_MS = 2000;

const catalog = useCatalogStore();
const list = ref<DeviceList | null>(null);
const error = ref<string | null>(null);
let timer: ReturnType<typeof setTimeout> | undefined;
let stopped = false;

async function load() {
  try {
    list.value = await api.devices();
    error.value = null;
    // A connection made for a device (`autoCreate`) isn't in the catalog yet.
    const known = new Set(catalog.connections.map((c) => c.id));
    if (list.value.devices.some((d) => d.heldBy !== undefined && !known.has(d.heldBy))) await catalog.refresh();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
  if (!stopped) timer = setTimeout(load, POLL_MS);
}

onMounted(load);
onBeforeUnmount(() => {
  stopped = true;
  clearTimeout(timer);
});

function pluginName(id: string): string {
  return catalog.manifest(id)?.name ?? id;
}

const tones: Record<NonNullable<DeviceEntry["handle"]>["state"], string> = {
  open: "good",
  reconnecting: "warn",
  error: "bad",
  closed: "muted",
};
</script>

<template>
  <section
    class="panel"
    aria-labelledby="devices-title"
  >
    <h2 id="devices-title">
      Devices
    </h2>
    <p
      v-if="error"
      class="error"
    >
      {{ error }}
    </p>
    <p
      v-if="list === null"
      class="muted"
    >
      Looking…
    </p>
    <p
      v-else-if="!list.available"
      class="muted"
    >
      This server doesn't look for USB devices.
    </p>
    <template v-else>
      <p
        v-if="list.scanError"
        class="error"
      >
        Can't list devices: {{ list.scanError.message }}
        <span
          v-if="list.scanError.hint"
          class="muted"
        >{{ list.scanError.hint }}</span>
      </p>
      <table v-if="list.devices.length > 0">
        <thead>
          <tr>
            <th>Device</th>
            <th>Kind</th>
            <th>Held by</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="d in list.devices"
            :key="d.id"
            :data-device="d.id"
          >
            <td>
              {{ deviceLabel(d) }}
              <div class="muted">
                <code>{{ d.id }}</code>
              </div>
            </td>
            <td class="muted">
              {{ d.kind.toUpperCase() }}
              <span v-if="d.vendorId !== undefined">{{ hex16(d.vendorId) }}:{{ hex16(d.productId) }}</span>
            </td>
            <td>
              <template v-if="d.heldBy !== undefined">
                <a :href="hrefOf({ tab: 'connections', conn: catalog.alias(d.heldBy) })"><code>{{ catalog.alias(d.heldBy) }}</code></a>
                <span
                  v-if="d.handle"
                  class="badge"
                  :class="tones[d.handle.state]"
                  :title="d.handle.error?.message"
                >{{ d.handle.state }}</span>
                <span
                  v-if="d.handle?.attached"
                  class="badge good"
                  title="A surface is attached on it"
                >surface</span>
                <div
                  v-if="d.handle?.error"
                  class="error"
                >
                  {{ d.handle.error.message }}
                </div>
              </template>
              <span
                v-else-if="d.offer?.kind === 'autoCreate'"
                class="muted"
              >Connecting with {{ pluginName(d.offer.plugin) }}…</span>
              <span
                v-else-if="d.offer?.kind === 'offer'"
                class="row offers"
              >
                <button
                  v-for="p in d.offer.plugins"
                  :key="p"
                  type="button"
                  class="secondary"
                  @click="emit('setUp', p, d.id)"
                >
                  Set up with {{ pluginName(p) }}
                </button>
              </span>
              <span
                v-else
                class="muted"
              >No plugin drives it</span>
            </td>
          </tr>
        </tbody>
      </table>
      <p
        v-else
        class="muted"
      >
        Nothing a plugin could drive is plugged in.
      </p>
    </template>
  </section>
</template>

<style scoped>
.offers {
  gap: 0.4rem;
  flex-wrap: wrap;
}

.badge {
  margin-left: 0.35rem;
}
</style>
