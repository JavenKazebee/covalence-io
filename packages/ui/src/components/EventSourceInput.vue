<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useAliases } from "../bindings/editing";
import {
  eventNames,
  eventSourceKind,
  idsOf,
  selectorText,
  type EventSourceKind,
  type PathId,
  type ShownProblem,
  type Trigger,
} from "../flows/model";
import { isRoot, parseRef, refText, type Json, type Ref } from "../sources/model";
import { useCatalogStore } from "../stores/catalog";
import { useSurfacesStore } from "../stores/surfaces";
import { controlEvents, eventHelp } from "../surfaces/model";

/**
 * Where an event trigger's events come from (dataflow §3.1), and which
 * events: a key on a layer or a surface, a connection by ID or plugin, or
 * any node by path, with `*` patterns.
 */
const props = defineProps<{ index: number; problems: ShownProblem[] }>();
const trigger = defineModel<Extract<Trigger, { kind: "event" }>>({ required: true });

const catalog = useCatalogStore();
const surfaces = useSurfacesStore();
const aliases = useAliases();

const label = computed(() => `Trigger ${props.index + 1}`);
const kind = computed(() => eventSourceKind(trigger.value.from));
const plugins = computed(() => catalog.connections.filter((c) => !c.builtIn));

const kindNames: Record<EventSourceKind, string> = {
  component: "A key on a page",
  surface: "A key on a surface",
  target: "A connection",
  path: "A path",
};

function setKind(next: EventSourceKind) {
  const t = trigger.value;
  switch (next) {
    case "component":
      t.from = { component: surfaces.layers[0]?.id ?? "" };
      t.event = "press";
      break;
    case "surface":
      t.from = { surface: surfaces.surfaces[0]?.id ?? "" };
      t.event = "press";
      break;
    case "target": {
      const conn = plugins.value[0];
      t.from = { target: conn?.id ?? "" };
      t.event = conn === undefined ? "" : (catalog.manifest(conn.plugin)?.events?.[0]?.id ?? "");
      break;
    }
    case "path":
      t.from = { path: "sys" };
      t.event = "startup";
      break;
  }
}

// ---- Controls: a list of IDs, or any ----

const from = computed(() => trigger.value.from as Record<string, unknown>);
const controls = computed(() => from.value.controls as Json | undefined);
const controlIds = computed(() => idsOf(controls.value));
const controlsText = computed(() =>
  controls.value === undefined ? "" : (controlIds.value?.join(", ") ?? selectorText(controls.value as Json, (c) => String(c))),
);

function setControls(text: string) {
  const ids = text
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s !== "");
  if (ids.length === 0) delete from.value.controls;
  else from.value.controls = ids.length === 1 ? ids[0] : ids;
}

const controlsOf = computed(() => {
  if (kind.value === "component") return surfaces.layer(String(from.value.component))?.controls.map((c) => c.id) ?? [];
  return [];
});

// ---- Targets: by connection or plugin ----

type TargetForm = "conn" | "plugin" | "other";
const targetSelector = computed(() => from.value.target as Json);
const targetForm = computed<TargetForm>(() => {
  const t = targetSelector.value;
  if (typeof t === "string") return "conn";
  if (t !== null && typeof t === "object" && !Array.isArray(t)) {
    if ("plugin" in t) return "plugin";
  }
  return "other";
});

function setTargetForm(form: TargetForm) {
  if (form === "conn") from.value.target = plugins.value[0]?.id ?? "";
  if (form === "plugin") from.value.target = { plugin: plugins.value[0]?.plugin ?? "" };
}

const pluginIds = computed(() => [...new Set(plugins.value.map((c) => c.plugin))]);

// ---- Paths: text for reserved roots, `{conn, path}` by alias, `*` patterns ----

function pathToText(path: Json): string {
  if (typeof path === "string") return path;
  if (path !== null && typeof path === "object" && !Array.isArray(path)) {
    if ("match" in path) return String(path.match);
    if ("conn" in path || "root" in path) return refText(path as unknown as Ref, aliases.value);
  }
  return JSON.stringify(path);
}

const pathText = ref(pathToText(from.value.path as Json));
const pathError = ref<string | null>(null);

watch(
  () => JSON.stringify(from.value.path),
  () => {
    if (pathError.value === null) pathText.value = pathToText(from.value.path as Json);
  },
);

function setPath(text: string) {
  pathText.value = text;
  const trimmed = text.trim();
  if (trimmed === "") {
    pathError.value = "Required";
    return;
  }
  if (trimmed.includes("*")) {
    pathError.value = null;
    from.value.path = { match: trimmed };
    return;
  }
  const root = trimmed.split(/[.[]/)[0] ?? "";
  if (isRoot(root)) {
    pathError.value = null;
    from.value.path = trimmed satisfies PathId;
    return;
  }
  const parsed = parseRef(trimmed, aliases.value);
  pathError.value = parsed.ok ? null : parsed.error;
  if (parsed.ok) from.value.path = parsed.value;
}

// ---- Event names ----

const namesText = computed(() => eventNames(trigger.value.event).join(", "));

function setNames(text: string) {
  const names = text
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s !== "");
  trigger.value.event = names.length === 1 ? names[0]! : names;
}

/** Names to suggest: a control's events, or the connection's. */
const suggested = computed<{ id: string; name: string }[]>(() => {
  if (kind.value === "component" || kind.value === "surface") return controlEvents.map((e) => ({ id: e, name: eventHelp[e] }));
  if (kind.value === "target" && targetForm.value === "conn") {
    const conn = catalog.connections.find((c) => c.id === targetSelector.value);
    return (conn === undefined ? [] : (catalog.manifest(conn.plugin)?.events ?? [])).map((e) => ({ id: e.id, name: e.name }));
  }
  if (kind.value === "path") {
    return [
      { id: "startup", name: "sys: the core started" },
      { id: "finished", name: "flow: a run finished" },
      { id: "failed", name: "flow: a run failed" },
      { id: "started", name: "flow: a run started" },
    ];
  }
  return [];
});

const listId = computed(() => `trigger-${props.index}-events`);
const fieldProblems = (field: string) => props.problems.filter((p) => p.path[2] === "from" && p.path[3] === field);
</script>

<template>
  <span
    class="event-source"
    :data-source="kind"
  >
    <select
      :value="kind"
      :aria-label="`${label} source`"
      @change="setKind(($event.target as HTMLSelectElement).value as EventSourceKind)"
    >
      <option
        v-for="(name, k) in kindNames"
        :key="k"
        :value="k"
      >
        {{ name }}
      </option>
    </select>

    <template v-if="kind === 'component'">
      <select
        v-model="from.component"
        :aria-label="`${label} layer`"
      >
        <option
          v-if="!surfaces.layer(String(from.component))"
          :value="from.component"
        >
          {{ from.component || "Pick a page" }}
        </option>
        <option
          v-for="l in surfaces.layers"
          :key="l.id"
          :value="l.id"
        >
          {{ l.name }}
        </option>
      </select>
    </template>
    <template v-else-if="kind === 'surface'">
      <select
        v-model="from.surface"
        :aria-label="`${label} surface`"
      >
        <option
          v-if="!surfaces.surfaces.some((s) => s.id === from.surface)"
          :value="from.surface"
        >
          {{ from.surface || "Pick a surface" }}
        </option>
        <option
          v-for="s in surfaces.surfaces"
          :key="s.id"
          :value="s.id"
        >
          {{ s.name }}
        </option>
      </select>
    </template>
    <template v-if="kind === 'component' || kind === 'surface'">
      <input
        :value="controlsText"
        :aria-label="`${label} controls`"
        :list="`${listId}-controls`"
        :disabled="controls !== undefined && controlIds === null"
        :title="controls !== undefined && controlIds === null ? 'Written as a selector; edit it in the flow’s JSON' : 'One or more control IDs, or empty for any'"
        placeholder="any control"
        class="mono"
        spellcheck="false"
        @change="setControls(($event.target as HTMLInputElement).value)"
      >
      <datalist :id="`${listId}-controls`">
        <option
          v-for="c in controlsOf"
          :key="c"
          :value="c"
        />
      </datalist>
    </template>

    <template v-else-if="kind === 'target'">
      <select
        :value="targetForm"
        :aria-label="`${label} target by`"
        @change="setTargetForm(($event.target as HTMLSelectElement).value as TargetForm)"
      >
        <option value="conn">
          Connection
        </option>
        <option value="plugin">
          Any of a plugin's
        </option>
        <option
          v-if="targetForm === 'other'"
          value="other"
        >
          {{ selectorText(targetSelector, (t) => JSON.stringify(t)) }}
        </option>
      </select>
      <select
        v-if="targetForm === 'conn'"
        v-model="from.target"
        :aria-label="`${label} connection`"
      >
        <option
          v-if="!plugins.some((c) => c.id === from.target)"
          :value="from.target"
        >
          {{ from.target || "Pick a connection" }}
        </option>
        <option
          v-for="c in plugins"
          :key="c.id"
          :value="c.id"
        >
          {{ c.alias }}
        </option>
      </select>
      <select
        v-else-if="targetForm === 'plugin'"
        v-model="(from.target as Record<string, string>).plugin"
        :aria-label="`${label} plugin`"
      >
        <option
          v-for="p in pluginIds"
          :key="p"
          :value="p"
        >
          {{ p }}
        </option>
      </select>
    </template>

    <span
      v-else-if="kind === 'path'"
      class="field"
    >
      <input
        :value="pathText"
        :aria-label="`${label} path`"
        placeholder="sys, flow.preService, surface.*.controls.*"
        :class="{ invalid: pathError }"
        class="mono"
        spellcheck="false"
        @input="setPath(($event.target as HTMLInputElement).value)"
      >
      <small
        v-if="pathError"
        class="error"
      >{{ pathError }}</small>
    </span>

    <input
      :value="namesText"
      :aria-label="`${label} event`"
      :list="listId"
      placeholder="press, or * for any"
      class="mono names"
      spellcheck="false"
      @change="setNames(($event.target as HTMLInputElement).value)"
    >
    <datalist :id="listId">
      <option
        v-for="e in suggested"
        :key="e.id"
        :value="e.id"
      >
        {{ e.name }}
      </option>
    </datalist>
    <small
      v-for="(p, j) in [...fieldProblems('component'), ...fieldProblems('controls'), ...fieldProblems('path'), ...fieldProblems('target')]"
      :key="j"
      :class="p.warning ? 'warn' : 'error'"
    >{{ p.message }}</small>
  </span>
</template>

<style scoped>
.event-source {
  display: inline-flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.4rem;
}

.mono {
  font-family: var(--mono);
  font-size: 0.92em;
}

.names {
  width: 10rem;
}

.field {
  display: inline-grid;
  gap: 0.15rem;
}

input.invalid {
  border-color: var(--bad);
}
</style>
