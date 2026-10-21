import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { api, type Connection, type Manifest, type NewConnection, type Plugin } from "../api";

/**
 * Plugins and connections over HTTP. Neither is on the WebSocket yet, so
 * `refresh` after anything that changes them, and when the plugin host comes
 * back (manifests are null until it has loaded the plugins).
 */
export const useCatalogStore = defineStore("catalog", () => {
  const plugins = ref<Plugin[]>([]);
  const connections = ref<Connection[]>([]);
  const loaded = ref(false);
  const error = ref<string | null>(null);

  async function refresh() {
    try {
      const [p, c] = await Promise.all([api.plugins(), api.connections()]);
      plugins.value = p;
      connections.value = c.sort((a, b) => a.alias.localeCompare(b.alias));
      error.value = null;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loaded.value = true;
    }
  }

  async function createConnection(req: NewConnection): Promise<Connection> {
    try {
      return await api.createConnection(req);
    } finally {
      // A 502 still saves the connection, so refresh either way.
      await refresh();
    }
  }

  const aliasById = computed(() => new Map(connections.value.map((c) => [c.id, c.alias])));

  /** A connection's alias, or its ID if it isn't known. */
  function alias(connId: string): string {
    return aliasById.value.get(connId) ?? connId;
  }

  function manifest(pluginId: string): Manifest | null {
    return plugins.value.find((p) => p.id === pluginId)?.manifest ?? null;
  }

  return { plugins, connections, loaded, error, refresh, createConnection, alias, manifest };
});
