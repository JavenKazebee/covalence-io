import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { api } from "../api";
import type { Page, SavedPage, SavedSurface, Surface } from "../surfaces/model";

/**
 * Surfaces and pages over HTTP. Neither is on the WebSocket, so this
 * refreshes after its own saves. What a surface shows is live state under
 * `surface` (surfaces spec §2).
 */
export const useSurfacesStore = defineStore("surfaces", () => {
  const surfaces = ref<Surface[]>([]);
  const pages = ref<Page[]>([]);
  const loaded = ref(false);
  const error = ref<string | null>(null);

  async function refresh() {
    try {
      const [s, p] = await Promise.all([api.surfaces(), api.pages()]);
      surfaces.value = s.sort((a, b) => a.name.localeCompare(b.name));
      pages.value = p.sort((a, b) => a.name.localeCompare(b.name));
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

  async function savePage(page: Page): Promise<SavedPage> {
    const saved = page.id === "" ? await api.createPage(page) : await api.updatePage(page);
    await refresh();
    return saved;
  }

  /** Deletes the page and its bindings, and clears it as any surface's start page. */
  async function removePage(id: string) {
    try {
      await api.deletePage(id);
    } finally {
      await refresh();
    }
  }

  const surfaceById = computed(() => new Map(surfaces.value.map((s) => [s.id, s])));
  const pageById = computed(() => new Map(pages.value.map((p) => [p.id, p])));
  const pageBySlug = computed(() => new Map(pages.value.map((p) => [p.slug, p])));

  return {
    surfaces,
    pages,
    loaded,
    error,
    refresh,
    saveSurface,
    removeSurface,
    savePage,
    removePage,
    surface: (id: string) => surfaceById.value.get(id),
    page: (id: string) => pageById.value.get(id),
    /** `surface.<slug>.page` holds a page's slug. */
    pageBySlug: (slug: string) => pageBySlug.value.get(slug),
    /** A surface's name, or its ID if it isn't known. */
    surfaceName: (id: string) => surfaceById.value.get(id)?.name ?? id,
    pageName: (id: string) => pageById.value.get(id)?.name ?? id,
  };
});
