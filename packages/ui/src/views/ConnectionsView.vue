<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { ConnMode } from "../api";
import DevicesPanel from "../components/DevicesPanel.vue";
import SchemaForm from "../components/SchemaForm.vue";
import TrafficPanel from "../components/TrafficPanel.vue";
import { deviceField, freeAlias } from "../devices/model";
import { fieldsOf, initialValues, parseValues } from "../forms/schema";
import { hrefOf } from "../nav";
import { useCatalogStore } from "../stores/catalog";

/** `trafficOf`: the connection whose traffic is open, by alias. */
defineProps<{ trafficOf?: string }>();

const catalog = useCatalogStore();

const loadedPlugins = computed(() => (catalog.plugins.filter((p) => p.manifest !== null)).filter((p) => !p.builtIn));
const pluginId = ref("");
const alias = ref("");
const mode = ref<ConnMode>("simulate");
const values = ref<Record<string, string | boolean>>({});
const errors = ref<Record<string, string>>({});
const formError = ref<string | null>(null);
const busy = ref(false);
const created = ref<string | null>(null);

const manifest = computed(() => catalog.manifest(pluginId.value));
const fields = computed(() => (manifest.value === null ? [] : fieldsOf(manifest.value.config.schema)));

// Pick the only plugin there is; reset the config form when the plugin changes.
watch(
  loadedPlugins,
  (plugins) => {
    if (pluginId.value === "" && plugins.length === 1 && plugins[0] !== undefined) pluginId.value = plugins[0].id;
  },
  { immediate: true },
);
watch(fields, (next) => (values.value = initialValues(next)), { immediate: true });

const newForm = ref<HTMLFormElement | null>(null);

/** Fills the new connection form for a device no connection holds. */
async function setUp(plugin: string, device: string) {
  pluginId.value = plugin;
  mode.value = "live";
  created.value = null;
  formError.value = null;
  errors.value = {};
  if (alias.value === "") alias.value = freeAlias(plugin, catalog.connections.map((c) => c.alias));
  // Let the form reset for the plugin before naming the device in it.
  await nextTick();
  const m = catalog.manifest(plugin);
  const field = m === null ? null : deviceField(m.config.schema);
  if (field !== null) values.value = { ...values.value, [field]: device };
  newForm.value?.scrollIntoView({ behavior: "smooth", block: "start" });
  newForm.value?.querySelector<HTMLElement>("#conn-alias")?.focus();
}

function pluginName(id: string): string {
  return catalog.manifest(id)?.name ?? id;
}

async function create() {
  formError.value = null;
  created.value = null;
  const parsed = parseValues(fields.value, values.value);
  const aliasError = /^[A-Za-z_][A-Za-z0-9_]*$/.test(alias.value) ? null : "Letters, digits and _, not starting with a digit";
  errors.value = parsed.ok ? {} : parsed.errors;
  if (aliasError !== null) errors.value = { ...errors.value, "@alias": aliasError };
  if (!parsed.ok || aliasError !== null) return;

  busy.value = true;
  try {
    const conn = await catalog.createConnection({
      plugin: pluginId.value,
      alias: alias.value,
      config: parsed.value,
      mode: mode.value,
    });
    created.value = conn.alias;
    alias.value = "";
    values.value = initialValues(fields.value);
  } catch (e) {
    formError.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <TrafficPanel
    v-if="trafficOf !== undefined"
    :conn="trafficOf"
  />
  <template v-else>
    <section class="panel">
      <h2>Connections</h2>
      <p
        v-if="catalog.error"
        class="error"
      >
        {{ catalog.error }}
      </p>
      <table v-if="catalog.connections.length > 0">
        <thead>
          <tr>
            <th>Alias</th>
            <th>Plugin</th>
            <th>Mode</th>
            <th>Config</th>
            <th>
              <span class="visually-hidden">Traffic</span>
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="conn in catalog.connections"
            :key="conn.id"
          >
            <td>
              <code v-if="conn.builtIn">{{ conn.alias }}</code>
              <a
                v-else
                :href="hrefOf({ tab: 'state', path: conn.alias })"
              ><code>{{ conn.alias }}</code></a>
            </td>
            <td>{{ pluginName(conn.plugin) }} <span class="muted">{{ conn.version }}</span></td>
            <td>
              <span
                class="badge"
                :class="conn.mode === 'live' ? 'good' : 'warn'"
              >{{ conn.mode }}</span>
              <span
                v-if="conn.directNetwork"
                class="badge"
                title="The plugin opens its own sockets, so Covalence can't show its traffic."
              >direct network</span>
            </td>
            <td>
              <span
                v-if="conn.builtIn"
                class="muted"
              >Built in: navigates surfaces and controls flows</span>
              <code v-else>{{ JSON.stringify(conn.config) }}</code>
            </td>
            <td>
              <a
                v-if="!conn.builtIn"
                :href="hrefOf({ tab: 'connections', conn: conn.alias })"
              >Traffic</a>
            </td>
          </tr>
        </tbody>
      </table>
      <p
        v-else-if="catalog.loaded"
        class="muted"
      >
        No connections yet.
      </p>
    </section>

    <DevicesPanel @set-up="setUp" />

    <section class="panel">
      <h2>New connection</h2>
      <p
        v-if="loadedPlugins.length === 0"
        class="muted"
      >
        Waiting for the plugin host to load plugins…
      </p>
      <form
        v-else
        ref="newForm"
        novalidate
        @submit.prevent="create"
      >
        <div class="field">
          <label for="conn-plugin">Plugin</label>
          <select
            id="conn-plugin"
            v-model="pluginId"
          >
            <option
              v-for="plugin in loadedPlugins"
              :key="plugin.id"
              :value="plugin.id"
            >
              {{ plugin.manifest?.name }} ({{ plugin.version }})
            </option>
          </select>
        </div>
        <div class="field">
          <label for="conn-alias">Alias<span class="required">*</span></label>
          <input
            id="conn-alias"
            v-model.trim="alias"
            placeholder="vh"
            spellcheck="false"
            autocomplete="off"
          >
          <small class="hint">The name its state and actions go by, e.g. <code>vh.outputs</code>.</small>
          <small
            v-if="errors['@alias']"
            class="error"
          >{{ errors["@alias"] }}</small>
        </div>
        <div class="field">
          <label for="conn-mode">Mode</label>
          <select
            id="conn-mode"
            v-model="mode"
          >
            <option value="simulate">
              Simulate
            </option>
            <option value="live">
              Live
            </option>
          </select>
        </div>
        <SchemaForm
          v-if="pluginId !== ''"
          v-model="values"
          :fields="fields"
          :errors="errors"
          :plugin="pluginId"
          id-prefix="conn-config"
        />
        <div class="row">
          <button
            type="submit"
            :disabled="busy || pluginId === ''"
          >
            {{ busy ? "Creating…" : "Create connection" }}
          </button>
          <span
            v-if="created"
            class="muted"
            role="status"
          >Created <code>{{ created }}</code>.</span>
          <span
            v-if="formError"
            class="error"
            role="alert"
          >{{ formError }}</span>
        </div>
      </form>
    </section>
  </template>
</template>

