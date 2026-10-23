# Spec: Composition

- **Status:** Draft v0.1
- **Related:** [ADR 0030](../adr/0030-composite-connections.md), [ADR 0031](../adr/0031-templates-instances-and-components.md), [ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md), [dataflow](dataflow.md), [surfaces](surfaces.md), [bindings](bindings.md), [flows](flows.md)

Users build bigger things out of smaller ones, and reuse them by giving them parameters, never by copying and editing. This spec covers the three ways to do that:

| | Builds | From |
|---|---|---|
| **Composite connection** (§1) | a device that doesn't exist on its own: "Camera A" | other connections' state, events and actions, and flows |
| **Template and instance** (§2) | many layers or composites that differ only in a few values | one definition with parameters |
| **Component** (§3) | a reusable control: a tally key, a camera strip | display bindings, helpers and flows for one or more controls |

**Instances take parameters, not overrides.** An instance can't change what's inside its template. If one copy really has to differ, "Detach" turns it into a plain entity with no link back. That keeps "what does this do?" answerable from one place.

**Markers.** Everything here is **(M9)** ([M9](../milestones/m9-composition.md)) unless marked **(Later)**.

---

## 1. Composite connections

A **composite** is a connection whose definition is a project entity, not a plugin. It has an alias, state, actions and events like any other connection, and it can implement interfaces, so every layer, binding and flow written against an interface works with it.

```ts
interface Connection {
  id: string; name: string; alias: string; enabled: boolean;
  plugin: string;                     // "core.composite" for a composite
  config: Value;                      // a CompositeDef when plugin is "core.composite"
  template?: string;                  // §2: an instance of another composite
  params?: { [name: string]: Source<Value> };
}

interface CompositeDef {
  params?: { [name: string]: ParamDef };
  state?: StateSources;               // a tree of sources: its state at <alias>.<path>
  connected?: Source<boolean>;        // default: true while every connection it reads is connected
  actions?: { [id: string]: CompositeAction };
  events?: { [id: string]: CompositeEvent };
  implements?: InterfaceImpls;        // as a plugin manifest's (ADR 0019)
}

type StateSources = { [segment: string]: Source<Value> | StateSources };

type CompositeAction = {
  name?: string;
  params: JSONSchema;
  result?: JSONSchema;
  idempotent?: boolean;
} & (
  | { flow: string }                  // run this flow; the action's params are its inputs
  | { forward: {                      // one action on another target
      target: Source<Target>; constraint?: TargetConstraint;
      action: string; params?: { [k: string]: Source<Value> } } }
);

interface CompositeEvent {
  from: EventSource; event: string | string[]; filter?: Expr;   // dataflow §3.1
  payload?: Source<Value>;            // `trigger` in scope; default the source event's payload
}

interface ParamDef extends StateSchemaNode {   // type, kind (target with interface…), description
  default?: Source<Value>;            // used by the template itself, and by instances that don't bind it
}
```

### 1.1 State
- **Each leaf is a cell** (dataflow §5) at `<alias>.<path>`: read-only, journaled with `cause` → what it read, and `stale` while anything it read is.
- `params` is in scope: `tally.program: {expr: "target(params.switcher).program == params.input"}`.
- **`connected`** is what `target(t).connected` and the UI show.

### 1.2 Actions
- **`flow`:** invoking the action starts a run of that flow, with `input` = the action's params and `trigger` = `{kind: "action", conn, call}`. The flow's `outputs` are the action's result, and a failed run is the action's error. `run.started` has `cause` → the `action.requested`, so `why` walks through the composite into the real device's action.
- **The flow must declare `inputs`** that accept the action's params, and `outputs` that match its `result` (flows §1); otherwise saving the composite fails (`badAction`). Its concurrency mode applies.
- **`forward`:** one action on another target, with params from sources. `params` (the composite's) and `input` (the action's params) are in scope.
- **Composite actions are `internal`** (core actions §4). The actions they cause go through the gate themselves, so simulate mode still stops the real device.

### 1.3 Events
- Each event is re-emitted at the composite's root, `<alias>:<id>`, with `cause` → the event it came from. `trigger` is in scope in `filter` and `payload`.

### 1.4 Interfaces
- `implements` maps the composite's members to an interface's, as a plugin manifest does. Conformance is checked when the composite is saved, with the same rules as a plugin's at load.
- A composite with `implements: {"camera.ptz@1": {}}` is selected by `{interface: "camera.ptz@1"}` like a real camera.

### 1.5 Limits
- **Composites can contain composites.** A state cycle is `cycle` (dataflow §5.3). An action that ends up invoking itself is stopped by the loop guard (flows §8.6), and calls nest at most 16 deep (`tooDeep`).
- **No collections yet.** A composite is one target. **(Later)** `items: Source<Value[]>` with `at`, for "every camera in this list".

### 1.6 Example

```yaml
name: Camera A
alias: camA
plugin: core.composite
config:
  params:
    ptz:      { kind: target, interface: camera.ptz@1 }
    switcher: { kind: target, interface: switcher@1 }
    input:    { type: integer }
    recorder: { kind: target, interface: recorder@1 }
  state:
    tally:
      program: { expr: "target(params.switcher).program == params.input" }
      preview: { expr: "target(params.switcher).preview == params.input" }
    recording: { expr: "target(params.recorder).recording" }
  actions:
    recallPreset:
      params: { type: object, properties: { preset: { type: integer } } }
      forward: { target: { ref: { scope: params, path: [ptz] } }, action: recallPreset,
                 params: { preset: { expr: "input.preset" } } }
    takeLive:
      params: { type: object }
      flow: 01JC0TAKE0LIVE0FLOW00000000
  implements:
    camera.ptz@1: {}
```

---

## 2. Templates and instances

- **Layers and composites can be templates:** any layer or composite that declares `params`. A template is usable itself, with its params' defaults. **(Later)** Surfaces and control groups.
- **An instance** is an entity of the same type with `template` and `params`, and only its own identity (`id`, `name`, `slug` or `alias`) besides. What it covers, binds and does comes from the template.
- **Layer instances keep their own placement fields:** `on`, `when`, `level` and `rank` (surfaces §5) say where an instance appears, not what it is.

```ts
interface Layer {                     // surfaces §1, with:
  params?: { [name: string]: ParamDef };       // makes it a template
  template?: string;                           // makes it an instance
  // an instance's params: values for the template's ParamDefs
}
```

- **Params are sources,** evaluated in the instance's scope, so an instance can follow state: `camera: {ref: {root: "vars", path: ["selectedCam"]}}`. Their values are state at `layer.<slug>.params.<name>` or under the composite's alias at `params.<name>`.
- **Editing a template changes every instance at once.** Saving a template whose new `params` an instance no longer satisfies fails with `badInstance`, listing them.
- **Bindings and helpers on a template layer** are evaluated once per instance, each with its own `params`. Their cells live under the instance: `layer.<instance slug>.controls.<id>.<name>`.
- **Events from an instance** carry the instance as `layer`. An event trigger on `{layer: <template>}` matches every instance, and its run gets that instance's `params` (dataflow §8). One flow handles every camera layer.
- **"Create from template"** makes an instance. **"Detach"** turns an instance into a plain copy with no link back. It's one-way.
- **Templates can't be instances.** One level keeps "where does this value come from?" to one hop.

---

## 3. Components

A **component** is a reusable control: what it looks like, how its input is shaped, and what it does, for one control or a few, with parameters. Plugins ship them as content with no code ([ADR 0025](../adr/0025-device-drivers-are-plugins.md)), replacing manifest `presets`.

```ts
interface Component {                 // project entity `component`, or manifest `components`
  id: string;
  name: string;
  slug: string;
  params?: { [name: string]: ParamDef };
  slots: LayerControl[];              // the controls it needs, by slot ID: usually one, "key"
  bindings: { [slot: string]: {
    props: { [prop in ControlProp]?: Source<Value> };   // bindings §1
    helpers?: Helper[];               // bindings §5
    gestures?: GestureTiming;         // surfaces §4.2
  } };
  flows?: OwnedFlow[];                // flows that belong to each placement
}

type OwnedFlow = Omit<Flow, "id"> & { id: string };   // ID unique within the component

interface Placement {                 // project entity `placement`
  id: string;
  slug: string;                       // state: layer.<layer slug>.parts.<slug>
  layer: string;
  component: string;
  controls: { [slot: string]: string };   // slot → the layer's control ID
  params?: { [name: string]: Source<Value> };
}
```

- **A placement owns its controls.** A control binding, a group or another placement on one of them is `duplicateControl`. Each slot's control must have the capabilities the slot declares (`doesNotFit`).
- **Params are bound in the layer's scope** (dataflow §8), so a placement on a camera layer passes the layer's camera on: `camera: {ref: {scope: "params", path: ["camera"]}}`.
- **Owned flows run per placement.** Their event triggers name controls by slot: `from: {slot: "key"}`. Inside them `params` is the placement's. Runs are journaled with `flow` = `<component ID>/<flow ID>` and `placement`, and their status is at `layer.<layer>.parts.<placement>.flows.<flow slug>`.
- **Helpers in a component** are cells at the placed control: `layer.<layer>.controls.<control>.<name>`, as for a plain binding.
- **Editing a component changes every placement.** "Detach" turns a placement into plain bindings and flows.
- **The key editor shows the component and its params** for a placed control, which answers "what does this key do?" from one place.
- **A control group can place a component per item** (surfaces §6): `component` and `params`, with `item`, `index` and `key` in scope for the params.

### 3.1 Example

```yaml
name: Tally key
slug: tallyKey
params:
  camera: { kind: target, interface: camera.ptz@1 }
  label:  { type: string }
slots:
  - { id: key, inputs: [momentary], outputs: [image] }
bindings:
  key:
    props:
      label: { ref: { scope: params, path: [label] } }
      background:
        use: rules
        rules:
          - { when: { expr: "target(params.camera).tally.program" }, value: "#d00" }
          - { when: { expr: "target(params.camera).tally.preview" }, value: "#0a0" }
        else: "#222"
flows:
  - id: select
    name: Select camera
    enabled: true
    triggers: [{ kind: event, from: { slot: key }, event: press }]
    concurrency: restart
    body:
      - { id: pick, kind: set, path: vars.selectedCam, value: params.camera }
```

---

## 4. Open questions

1. Should components nest (a camera strip made of tally keys), or is one level enough until real projects ask?
2. Should an instance be allowed to add bindings to controls its template leaves unbound, or is that a different layer stacked over it?
3. Do composites need their own run history and traffic view in the UI, or is the journal enough?
