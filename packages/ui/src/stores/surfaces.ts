import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { api } from "../api";
import type { Layer, SavedLayer, SavedSurface, Surface } from "../surfaces/model";

/**
 * Surfaces and layers over HTTP. Neither is on the WebSocket, so this
 * refreshes after its own saves. What a surface shows is live state under
 * `surface` (surfaces spec §2).
 */
export const useSurfacesStore = defineStore("surfaces", () => {
  const surfaces = ref<Surface[]>([]);
  const layers = ref<Layer[]>([]);
  const loaded = ref(false);
  const error = ref<string | null>(null);

  async function refresh() {
    try {
      const [s, p] = await Promise.all([api.surfaces(), api.layers()]);
      surfaces.value = s.sort((a, b) => a.name.localeCompare(b.name));
      layers.value = p.sort((a, b) => a.name.localeCompare(b.name));
      error.value = null;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loaded.value = true;
    }
  }

  /** Creates the surface when it has no ID, else replaces it. Throws `ApiError` 422 with problems. */
  async function saveSurface(surface: Surface): Promise<SavedSurface> {
    const saved = surface.id === "" ? await api.createSurface(surface) : await api.updateSurface(surface);
    await refresh();
    return saved;
  }

  async function removeSurface(id: string) {
    try {
      await api.deleteSurface(id);
    } finally {
      await refresh();
    }
  }

  async function saveLayer(layer: Layer): Promise<SavedLayer> {
    const saved = layer.id === "" ? await api.createLayer(layer) : await api.updateLayer(layer);
    await refresh();
    return saved;
  }

  /** Deletes the layer and its bindings, and clears it as any surface's start layer. */
  async function removeLayer(id: string) {
    try {
      await api.deleteLayer(id);
    } finally {
      await refresh();
    }
  }

  const surfaceById = computed(() => new Map(surfaces.value.map((s) => [s.id, s])));
  const layerById = computed(() => new Map(layers.value.map((p) => [p.id, p])));
  const layerBySlug = computed(() => new Map(layers.value.map((p) => [p.slug, p])));

  return {
    surfaces,
    layers,
    loaded,
    error,
    refresh,
    saveSurface,
    removeSurface,
    saveLayer,
    removeLayer,
    surface: (id: string) => surfaceById.value.get(id),
    layer: (id: string) => layerById.value.get(id),
    /** `surface.<slug>.current.main` holds a layer's slug. */
    layerBySlug: (slug: string) => layerBySlug.value.get(slug),
    /** A surface's name, or its ID if it isn't known. */
    surfaceName: (id: string) => surfaceById.value.get(id)?.name ?? id,
    layerName: (id: string) => layerById.value.get(id)?.name ?? id,
  };
});
