<script setup lang="ts">
import { useCatalogStore } from "../stores/catalog";
import { varsRef, type Sink } from "../variables/model";

/**
 * Where a cell's value goes (dataflow spec §7): a writable variable, or an
 * action's parameter, rate-limited. Used by variables and helpers.
 */
const props = defineProps<{ idPrefix: string }>();
const sinks = defineModel<Sink[]>({ required: true });

const catalog = useCatalogStore();

type ActionSink = Extract<Sink, { action: unknown }>["action"];

function add(form: "set" | "action") {
  const conn = catalog.connections.find((c) => !c.builtIn)?.id ?? "";
  const sink: Sink =
    form === "set" ? { set: { root: "vars", path: [""] } } : { action: { target: { conn }, action: "", param: "" } };
  sinks.value = [...sinks.value, sink];
}
function replace(i: number, sink: Sink) {
  sinks.value = sinks.value.map((s, j) => (j === i ? sink : s));
}
function remove(i: number) {
  sinks.value = sinks.value.filter((_, j) => j !== i);
}
const setPath = (s: Sink) => ("set" in s ? `vars.${String(s.set.path[0] ?? "")}` : "");
const actionOf = (s: Sink): ActionSink | null => ("action" in s ? s.action : null);
const connOf = (s: Sink) => {
  const t = actionOf(s)?.target;
  return typeof t === "object" && t !== null && "conn" in t ? String(t.conn) : typeof t === "string" ? t : "";
};
const manifestOf = (conn: string) => {
  const c = catalog.connections.find((x) => x.id === conn);
  return c === undefined ? null : catalog.manifest(c.plugin);
};
const actionsOf = (conn: string) => manifestOf(conn)?.actions ?? [];
const paramsOf = (conn: string, action: string) =>
  Object.keys(actionsOf(conn).find((a) => a.id === action)?.params.properties ?? {});
function edit(i: number, s: Sink, fields: Partial<ActionSink>) {
  const a = actionOf(s);
  if (a !== null) replace(i, { action: { ...a, ...fields } });
}
const id = (i: number, part: string) => `${props.idPrefix}-${i}-${part}`;
</script>

<template>
  <p
    v-if="sinks.length === 0"
    class="muted"
  >
    Nowhere. A sink keeps a variable or an action in step with this value, sending on each change.
  </p>
  <div
    v-for="(s, i) in sinks"
    :key="i"
    class="row sink"
  >
    <div
      v-if="'set' in s"
      class="field"
    >
      <label :for="id(i, 'set')">Sets</label>
      <input
        :id="id(i, 'set')"
        :value="setPath(s)"
        autocomplete="off"
        @change="replace(i, { set: varsRef(($event.target as HTMLInputElement).value) ?? s.set })"
      >
    </div>
    <template v-else>
      <div class="field">
        <label :for="id(i, 'conn')">Connection</label>
        <select
          :id="id(i, 'conn')"
          :value="connOf(s)"
          @change="edit(i, s, { target: { conn: ($event.target as HTMLSelectElement).value }, action: '', param: '' })"
        >
          <option
            v-for="c in catalog.connections"
            :key="c.id"
            :value="c.id"
          >
            {{ c.alias }}
          </option>
        </select>
      </div>
      <div class="field">
        <label :for="id(i, 'action')">Action</label>
        <select
          :id="id(i, 'action')"
          :value="actionOf(s)?.action"
          @change="edit(i, s, { action: ($event.target as HTMLSelectElement).value, param: '' })"
        >
          <option
            v-for="a in actionsOf(connOf(s))"
            :key="a.id"
            :value="a.id"
          >
            {{ a.name }}
          </option>
        </select>
      </div>
      <div class="field">
        <label :for="id(i, 'param')">Value goes to</label>
        <select
          :id="id(i, 'param')"
          :value="actionOf(s)?.param"
          @change="edit(i, s, { param: ($event.target as HTMLSelectElement).value })"
        >
          <option
            v-for="p in paramsOf(connOf(s), actionOf(s)?.action ?? '')"
            :key="p"
            :value="p"
          >
            {{ p }}
          </option>
        </select>
      </div>
      <div class="field">
        <label :for="id(i, 'rate')">At most every</label>
        <input
          :id="id(i, 'rate')"
          :value="actionOf(s)?.rateLimit ?? ''"
          placeholder="20ms"
          autocomplete="off"
          @change="edit(i, s, { rateLimit: ($event.target as HTMLInputElement).value.trim() || undefined })"
        >
      </div>
    </template>
    <button
      type="button"
      class="secondary"
      @click="remove(i)"
    >
      Remove
    </button>
  </div>
  <div class="row">
    <button
      type="button"
      class="secondary"
      @click="add('set')"
    >
      Add: set a variable
    </button>
    <button
      type="button"
      class="secondary"
      @click="add('action')"
    >
      Add: send an action
    </button>
  </div>
</template>

<style scoped>
.sink {
  align-items: flex-end;
}
</style>
