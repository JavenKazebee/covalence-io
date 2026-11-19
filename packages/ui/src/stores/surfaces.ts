import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import { api, relative } from "../api";
import type { ControlBinding, SavedBinding } from "../bindings/model";
import {
  bindingProblems,
  newBindingId,
  type Component,
  type SavedComponent,
  type SavedSurface,
  type Surface,
} from "../surfaces/model";

/**
 * Surfaces and components over HTTP. Neither is on the WebSocket, so this
 * refreshes after its own saves. What a surface shows is live state under
 * `surface` (surfaces spec §3).
 */
export const useSurfacesStore = defineStore("surfaces", () => {
  const surfaces = ref<Surface[]>([]);
  // Replaced whole on each load; shallow, as sources nest deeply.
  const components = shallowRef<Component[]>([]);
  const loaded = ref(false);
  const error = ref<string | null>(null);

  async function refresh() {
    try {
      const [s, c] = await Promise.all([api.surfaces(), api.components()]);
      surfaces.value = s.sort((a, b) => a.name.localeCompare(b.name));
      components.value = c.sort((a, b) => a.name.localeCompare(b.name));
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

  /** Creates the component when it has no ID, else replaces it, bindings and all. */
  async function saveComponent(component: Component): Promise<SavedComponent> {
    const saved = await api.saveComponent(component);
    await refresh();
    return saved;
  }

  /** Deletes the component, and clears it as any surface's start. */
  async function removeComponent(id: string) {
    try {
      await api.deleteComponent(id);
    } finally {
      await refresh();
    }
  }

  /**
   * Adds a binding to its component, or replaces the one with its ID, and
   * puts the component back (composition spec §7.1). One with no ID gets one.
   * A 422's problems are the binding's, relative to it.
   */
  async function saveBinding(component: string, binding: ControlBinding): Promise<SavedBinding> {
    const c = await api.component(component);
    const bindings = [...(c.bindings ?? [])];
    const id = binding.id === "" ? newBindingId(c) : binding.id;
    const at = bindings.findIndex((b) => b.id === id);
    const index = at === -1 ? bindings.length : at;
    bindings[index] = { ...binding, id };
    try {
      const saved = await saveComponent({ ...c, bindings });
      const stored = (saved.component.bindings ?? []).find((b) => b.id === id)!;
      return { binding: stored, warnings: bindingProblems(saved.warnings, index) };
    } catch (e) {
      throw relative(e, (problems) => bindingProblems(problems, index));
    }
  }

  /** Takes a binding out of its component. */
  async function removeBinding(component: string, binding: string) {
    const c = await api.component(component);
    await saveComponent({ ...c, bindings: (c.bindings ?? []).filter((b) => b.id !== binding) });
  }

  const surfaceById = computed(() => new Map(surfaces.value.map((s) => [s.id, s])));
  const componentById = computed(() => new Map(components.value.map((c) => [c.id, c])));
  const componentBySlug = computed(() => new Map(components.value.map((c) => [c.slug, c])));

  return {
    surfaces,
    components,
    /** The components shown on surfaces: pages, shifts, popovers. */
    shown: computed(() => components.value.filter((c) => c.show !== undefined)),
    loaded,
    error,
    refresh,
    saveSurface,
    removeSurface,
    saveComponent,
    removeComponent,
    saveBinding,
    removeBinding,
    surface: (id: string) => surfaceById.value.get(id),
    component: (id: string) => componentById.value.get(id),
    /** `surface.<slug>.current.main` holds a component's slug. */
    componentBySlug: (slug: string) => componentBySlug.value.get(slug),
    /** A surface's name, or its ID if it isn't known. */
    surfaceName: (id: string) => surfaceById.value.get(id)?.name ?? id,
    componentName: (id: string) => componentById.value.get(id)?.name ?? id,
  };
});
