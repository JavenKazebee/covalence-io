import { computed, inject, onBeforeUnmount, provide, reactive, type InjectionKey } from "vue";
import { useCatalogStore } from "../stores/catalog";
import type { Aliases } from "./model";

/** Connections by alias, as the binding editor names them. */
export function useAliases() {
  const catalog = useCatalogStore();
  return computed<Aliases>(() => {
    const byId = new Map(catalog.connections.map((c) => [c.id, c.alias]));
    const byAlias = new Map(catalog.connections.map((c) => [c.alias, c.id]));
    return { alias: (id) => byId.get(id), conn: (alias) => byAlias.get(alias) };
  });
}

/**
 * Fields whose text doesn't parse, so the binding editor won't save a
 * binding that still holds what they last parsed to.
 */
const invalidKey: InjectionKey<Set<symbol>> = Symbol("invalid fields");

export function provideInvalid(): Set<symbol> {
  const invalid = reactive(new Set<symbol>());
  provide(invalidKey, invalid);
  return invalid;
}

/** Reports whether this field's text parses. */
export function useValidity(): (ok: boolean) => void {
  const invalid = inject(invalidKey, null);
  const me = Symbol("field");
  onBeforeUnmount(() => invalid?.delete(me));
  return (ok) => {
    if (ok) invalid?.delete(me);
    else invalid?.add(me);
  };
}
