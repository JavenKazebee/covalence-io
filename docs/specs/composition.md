# Spec: Composition

- **Status:** Draft v0.2
- **Related:** [ADR 0032](../adr/0032-one-template-primitive.md), [ADR 0030](../adr/0030-composite-connections.md), [ADR 0031](../adr/0031-templates-instances-and-components.md), [ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md), [dataflow](dataflow.md), [surfaces](surfaces.md), [bindings](bindings.md), [flows](flows.md)

Users build bigger things out of smaller ones, and reuse them by giving them parameters, never by copying and editing. There is **one way to do that**: a **template** declares `params`, and an **instance** binds them (§1). It works the same for four kinds of entity:

| Kind | A template is | An instance is | Its body (§2) | Section |
|---|---|---|---|---|
| **Composite connection** | a connection built from others: "Camera" | a connection: "Camera A" | state, actions, events, owned flows | §6 |
| **Layer** | a layer with `params`: "Camera control" | a layer | control bindings, groups, placements, owned flows | §7 |
| **Component** | a reusable control: "Tally key" | a **placement** on a layer's controls | slots, bindings, helpers, parts, owned flows | §8 |
| **Flow** | a flow with `params`: "Recorder watchdog" | a flow | steps | §9 |

**Instances take parameters, not overrides.** An instance can't change, add to or remove anything inside its template. If one copy really has to differ, "Detach" turns it into a plain entity with no link back. That keeps "what does this do?" answerable from one place.

**Markers.** Everything here is **(M9)** ([M9](../milestones/m9-composition.md)) unless marked **(Later)**.

---

## 1. Templates and instances

```ts
interface ParamDef extends StateSchemaNode {   // type, kind (target with interface…), description
  default?: Source<Value>;            // used by the template itself, and by instances that don't bind it
  required?: boolean;                 // no default and required: the template is abstract
}

interface TemplateFields {            // on a layer, a composite's CompositeDef, a component, a flow
  params?: { [name: string]: ParamDef };
}

interface InstanceFields {            // on a layer, a connection, a placement, a flow
  template: string;                   // the template's ID
  params?: { [name: string]: Source<Value> };  // bound in the placer's scope (dataflow §8)
  forEach?: ForEach;                  // §4; not on placements, where a control group does it
}
```

- **A template is any entity of the four kinds that declares `params`.** An instance is an entity of the same kind with `template`, plus its own identity (`id`, `name`, `slug` or `alias`) and the fields that say where it appears:
  - a layer instance: `on`, `when`, `level`, `rank` (surfaces §5)
  - a placement: `layer` and `controls` (§8)
  - a flow instance: `enabled`, `owner` (§2)
  - a composite instance: `enabled`
- **An entity is a template or an instance, never both.** A template can't derive from another template (`templateIsInstance`). It can contain instances (§3).
- **A template is usable itself where its kind can be** (a layer is shown, a composite connects, a flow runs), with its params' defaults. One with a `required` param and no default is **abstract**: it can only be used through instances, and using it directly is `unboundParam`. A component is always abstract, since it needs controls.
- **Params are sources** ([dataflow §1](dataflow.md)), evaluated reactively in the placer's scope, so an instance can follow state: `camera: {ref: {root: "vars", path: ["selectedCam"]}}`. A param's value is state at `<home>.params.<name>`, where `<home>` is the instance's home (§1.1).
- **Editing a template changes every instance at once.** A change an instance can no longer satisfy (a new required param, a type an existing binding doesn't match, a slot that no longer fits) is refused with `badInstance`, listing the instances.
- **One module, `covalence_core::compose`,** binds, validates and evaluates instances of every kind. The UI shows one "instance of X, with these params" panel for all of them.

### 1.1 Homes

Each instance has a home in the state tree. Its params, cells and owned flows' status are under it.

| Kind | Home | Per item (§4) |
|---|---|---|
| Composite | `<alias>` | `<alias>.items[<key>]` |
| Layer | `layer.<slug>` | `layer.<slug>.items[<key>]` |
| Placement | `layer.<layer slug>.parts.<slug>` | in a control group: `layer.<layer slug>.groups.<group>.items[<key>]` |
| Flow | `flow.<slug>` | `flow.<slug>.items[<key>]` |
| A nested instance (§3) | `<container's home>.parts.<slug>` | `<container's home>.parts.<slug>.items[<key>]` |

---

## 2. Bodies and owned flows

- **A template's body** is the entity plus everything that belongs to it. Layers own their control bindings, control groups and placements (by their `layer` field) and their owned flows (by `owner`). Composites and components embed theirs, so a plugin can ship a component whole.
- **The body is evaluated once per instance,** with that instance's `params`. A layer template's bindings and helpers have their cells under each instance: `layer.<instance slug>.controls.<id>.<name>`.
- **Owned flows** are flows that belong to a layer, a composite or a component:

  ```ts
  interface Flow {                    // flows §1, with:
    owner?: string;                   // a layer or composite ID; components embed theirs as `flows`
  }
  ```

  - They run **per instance** of their owner. Their status is at `<owner's home>.flows.<slug>`, and their events at `<owner's home>.flows.<slug>:finished`.
  - Inside them, `params` is the owner instance's (dataflow §8).
  - Their event triggers can name the owner's parts: `from: {slot: "key"}` in a component, `from: {controls: "k0"}` (the owning layer implied) in a layer.
  - An event trigger on the owner matches events from every instance, and each run belongs to the instance the event came from.
- **Free-standing flows see only their own `params`.** A free-standing flow triggered by a control event gets `trigger.layer` and `trigger.instance` (§5), not the layer's params. A flow that should handle every instance of a layer template is owned by it.

### 2.1 Params and inputs
Flows have both, with the same `ParamDef` shape, binding shape and editor:

| | `params` | `inputs` |
|---|---|---|
| Bound | where an instance is placed | each time it's invoked: `call`, a composite action, the API, OSC |
| Evaluated | reactively, for as long as the instance exists | once, when the run starts |
| In scope as | `params` | `input` |
| Gives | an identity and a home (§1.1) | a run, which already has a handle and a journal identity |

---

## 3. Nesting

- **A template's body can contain instances of other templates:**

  | Container | Can contain |
  |---|---|
  | Layer | placements (§8), control groups placing a component per item, flow instances it owns |
  | Component | component instances (`parts`, §8.1), flow instances it owns |
  | Composite | flow instances it owns. **(Later)** composite instances |
  | Flow | nothing; a flow uses others with `call` |

- **A nested instance binds its params in its container's scope.** `params` is always the innermost instance's own, so a value passes down one visible binding at a time: a "Camera strip" component's tally part gets `camera: {ref: {scope: "params", path: ["camera"]}}`, the strip's own camera.
- **No recursion.** A template that contains itself, directly or through others, can't be saved (`recursiveTemplate`). Instances nest at most 16 deep (`tooDeep`).
- **Editing a nested template** changes it everywhere it's used, at any depth, and the `badInstance` check covers every level.

---

## 4. One instance per item

```ts
type ForEach = (
  | { items: Source<Value[]> }               // a live list: {expr: "target(vars.selectedCam).isos"}
  | { targets: Selector<Target> }            // every target the selector picks: {interface: "recorder@1"}
) & {
  key?: Expr;                                // each item's identity; default the item, or a target's ID form
};
```

- **`forEach` makes one instance per item.** `item`, `index` and `key` are in scope while that instance's `params` are bound, and only there. Inside the body, the item is whatever param it was bound to: `params: {recorder: {expr: "item"}}`.
- **Per-item instances live at `<home>.items[<key>]`** (§1.1), so a reorder doesn't reset them, and other state can read them by key.
- **They follow the list.** An added item creates an instance, with its cells starting from `init`. A removed item ends one: its cells are dropped, and its owned flows' runs are cancelled with `removed`. Both are journaled, `instance.added` and `instance.removed` with `{instance}`, and `cause` → the patch or the domain change that changed the list.
- **They're never saved.** The entity is the one instance with `forEach`. Its items come from state each time it's loaded.
- **At most 256 items.** Extra items aren't instanced, and the instance reports `tooMany`.
- **Per kind:**
  - **A composite with `forEach` is a connection with a collection** (§6.4): each item is a target.
  - **A layer with `forEach`** is one layer per item, each navigated by `{layer, item: key}` ([core actions §3.1](core-actions.md)).
  - **A flow with `forEach`** is one flow per item, each with its own runs and concurrency: a watchdog per recorder.
  - **Placements per item** are what a control group does (surfaces §6), since it also decides which control each item takes, and scrolls. Its `use` is an instance without `forEach`.
- **`forEach` instances nest.** Inside a per-item instance, a nested `forEach` sees its own `item`, not the outer one's. The outer item reaches it through a param.

---

## 5. Identity, validation and Detach

- **Every journal entry made inside an instance carries `instance`:**

  ```ts
  type InstanceChain = { template: string; instance: string; key?: PathSegment }[];   // outermost first
  ```

  Runs, steps, actions, cells' patches and events made inside an instance all carry it, and an event's `trigger.instance` is it. It replaces the separate `placement` field, the `<component>/<flow>` run identity and the layer-as-instance of ADR 0031. The timeline filters by it, and `why` shows it: "Tally key, camera = Cam B, in Cameras[camB]".
- **Validation** (errors block saving):
  - `badInstance`: a required param unbound, a binding whose type doesn't match the `ParamDef`, a placement whose controls don't fit its slots. When a template is saved, every instance of it at every depth is checked, and the error lists the ones that fail.
  - `unknownTemplate`, `templateIsInstance`, `recursiveTemplate`, `tooDeep`.
  - `unboundParam`: an abstract template used directly.
  - `tooMany` is reported at run time, not at save.
- **Detach** works the same for every kind. It turns an instance into a plain entity with its template's body copied in and its params left as they were bound, as sources. A `forEach` instance detaches to one plain entity per current item, with each item's value written in as a literal. The editor warns that they won't follow the list anymore. Detach is one-way.

---

## 6. Composite connections

A **composite** is a connection whose definition is a project entity, not a plugin. It has an alias, state, actions and events like any other connection, and it can implement interfaces, so every layer, binding and flow written against an interface works with it.

```ts
interface Connection {
  id: string; name: string; alias: string; enabled: boolean;
  plugin: string;                     // "core.composite" for a composite
  config: Value;                      // a CompositeDef when plugin is "core.composite" and it's not an instance
  template?: string;                  // §1: an instance of another composite
  params?: { [name: string]: Source<Value> };
  forEach?: ForEach;                  // §4: a collection of composites
}

interface CompositeDef extends TemplateFields {
  state?: StateSources;               // a tree of sources: its state at <home>.<path>
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
  | { flow: string }                  // run this owned flow; the action's params are its inputs
  | { forward: {                      // one action on another target
      target: Source<Target>; constraint?: TargetConstraint;
      action: string; params?: { [k: string]: Source<Value> } } }
);

interface CompositeEvent {
  from: EventSource; event: string | string[]; filter?: Expr;   // dataflow §3.1
  payload?: Source<Value>;            // `trigger` in scope; default the source event's payload
}
```

### 6.1 State
- **Each leaf is a cell** (dataflow §5) at `<home>.<path>`: read-only, journaled with `cause` → what it read, and `stale` while anything it read is.
- `params` is in scope: `tally.program: {expr: "target(params.switcher).program == params.input"}`.
- **`connected`** is what `target(t).connected` and the UI show.

### 6.2 Actions
- **`flow`:** invoking the action starts a run of that flow, which the composite owns (§2), with `input` = the action's params, `params` = the composite instance's, and `trigger` = `{kind: "action", conn, call}`. The flow's `outputs` are the action's result, and a failed run is the action's error. `run.started` has `cause` → the `action.requested`, so `why` walks through the composite into the real device's action.
- **The flow must declare `inputs`** that accept the action's params, and `outputs` that match its `result` (flows §1). Otherwise saving the composite fails (`badAction`). Its concurrency mode applies, per instance.
- **`forward`:** one action on another target, with params from sources. `params` (the composite's) and `input` (the action's params) are in scope.
- **Composite actions are `internal`** (core actions §4). The actions they cause go through the gate themselves, so simulate mode still stops the real device.

### 6.3 Events
- Each event is re-emitted at the instance's home, `<alias>:<id>` (or `<alias>.items[<key>]:<id>`), with `cause` → the event it came from. `trigger` is in scope in `filter` and `payload`.

### 6.4 Collections
- **A composite with `forEach` is one connection with a collection.** Each item is a target, `{conn, key: ["items", <key>]}`, with its state at `<alias>.items[<key>]`.
- **Its interfaces are implemented per item,** through `at: ["items"]` ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)), so `{interface: "camera.ptz@1"}` selects each item like a real camera.
- **Actions and events are per item:** an action on an item's target runs with that item's `params`, and an item's events carry its key in `instance`.

### 6.5 Interfaces
- `implements` maps the composite's members to an interface's, as a plugin manifest does. Conformance is checked when the composite is saved, with the same rules as a plugin's at load.
- A composite with `implements: {"camera.ptz@1": {}}` is selected by `{interface: "camera.ptz@1"}` like a real camera.

### 6.6 Limits
- **Composites combine others by reference,** through target params. A state cycle is `cycle` (dataflow §5.3). An action that ends up invoking itself is stopped by the loop guard (flows §8.6), and calls nest at most 16 deep (`tooDeep`).
- **(Later)** A composite contains composite instances as `parts`.

### 6.7 Example

A template, and one instance per camera in a list:

```yaml
name: Camera
alias: camera
plugin: core.composite
config:
  params:
    ptz:      { kind: target, interface: camera.ptz@1, required: true }
    switcher: { kind: target, interface: switcher@1, required: true }
    input:    { type: integer, required: true }
    recorder: { kind: target, interface: recorder@1 }
  state:
    tally:
      program: { expr: "target(params.switcher).program == params.input" }
      preview: { expr: "target(params.switcher).preview == params.input" }
    recording: { expr: "target(params.recorder)?.recording ?? false" }
  actions:
    recallPreset:
      params: { type: object, properties: { preset: { type: integer } } }
      forward: { target: { ref: { scope: params, path: [ptz] } }, action: recallPreset,
                 params: { preset: { expr: "input.preset" } } }
    takeLive:
      params: { type: object }
      flow: 01JC0TAKE0LIVE0FLOW00000000   # a flow with owner: this composite
  implements:
    camera.ptz@1: {}
---
name: Cameras
alias: cams
plugin: core.composite
template: <Camera's ID>
forEach: { items: { ref: { root: vars, path: [cameraRig] } }, key: "item.name" }
params:
  ptz:      { expr: "item.ptz" }
  switcher: { ref: { root: vars, path: [switcher] } }
  input:    { expr: "item.input" }
```

`cams.items["Cam A"].tally.program` follows the switcher, and `{conn: cams, key: ["items", "Cam A"]}` is a `camera.ptz@1` target.

---

## 7. Layer templates

```ts
interface Layer {                     // surfaces §1, with:
  params?: { [name: string]: ParamDef };       // a template (§1)
  template?: string;                           // or an instance; `params` are then Sources
  forEach?: ForEach;                           // §4: one layer per item
}
```

- **Layer instances keep their own placement fields:** `on`, `when`, `level` and `rank` (surfaces §5) say where an instance appears, not what it is. In a `forEach` instance they're evaluated per item, with `item` in scope.
- **Bindings, helpers, groups and placements on a template layer** are evaluated once per instance, each with its own `params`.
- **Events from an instance** carry the instance in `trigger.instance`, and its layer as `trigger.layer`. A flow the template owns (§2) handles them with that instance's `params`: one flow for every camera layer.
- **An instance can't add bindings** to controls its template leaves unbound. A layer stacked over it does that (surfaces §5).
- **"Create from template"** makes an instance. **"Detach"** turns one into a plain layer with copies of the template's bindings, groups, placements and owned flows.

---

## 8. Components

A **component** is a reusable control: what it looks like, how its input is shaped, and what it does, for one control or a few, with parameters. It's always a template, and its instances are **placements**. Plugins ship them as content with no code ([ADR 0025](../adr/0025-device-drivers-are-plugins.md)), replacing manifest `presets`.

```ts
interface Component extends TemplateFields {   // project entity `component`, or manifest `components`
  id: string;
  name: string;
  slug: string;
  slots: LayerControl[];              // the controls it needs, by slot ID: usually one, "key"
  bindings?: { [slot: string]: {
    props: { [prop in ControlProp]?: Source<Value> };   // bindings §1
    helpers?: Helper[];               // bindings §5
    gestures?: GestureTiming;         // surfaces §4.2
  } };
  parts?: { [slug: string]: Part };   // §8.1: components inside this one
  flows?: OwnedFlow[];                // flows owned by each placement (§2)
}

type OwnedFlow = Omit<Flow, "id" | "owner"> & { id: string };   // ID unique within the component

interface Placement extends InstanceFields {   // project entity `placement`; `template` is a component ID
  id: string;
  slug: string;                       // home: layer.<layer slug>.parts.<slug>
  layer: string;
  controls: { [slot: string]: string };   // slot → the layer's control ID
}
```

- **A placement owns its controls.** A control binding, a group or another placement on one of them is `duplicateControl`. Each slot's control must have the capabilities the slot declares (`badInstance`, with `doesNotFit`).
- **Params are bound in the layer's scope** (dataflow §8), so a placement on a camera layer passes the layer's camera on: `camera: {ref: {scope: "params", path: ["camera"]}}`.
- **Owned flows run per placement.** Their event triggers name controls by slot: `from: {slot: "key"}`. Inside them `params` is the placement's. Their status is at `<placement's home>.flows.<flow slug>`.
- **Helpers in a component** are cells at the placed control: `layer.<layer>.controls.<control>.<name>`, as for a plain binding.
- **The key editor shows the component and its params** for a placed control, with the chain of containers for a nested one. That answers "what does this key do?" from one place.
- **A control group places a component per item** (surfaces §6): `use: {template, params}`, with `item`, `index` and `key` in scope for the params.

### 8.1 Parts

```ts
interface Part {
  template: string;                   // a component
  params?: { [name: string]: Source<Value> };   // bound in this component's scope
  slots: { [innerSlot: string]: string };       // the part's slot → one of this component's slots
}
```

- **A part is a component instance inside another component.** Its slots map onto the outer component's slots, so placing the outer component places every part.
- **Each slot is covered once:** by a binding of the outer component, or by one part (`duplicateControl`).
- A part's home is `<placement's home>.parts.<part slug>`, and its helpers' cells are at the placed control, as for any component.

### 8.2 Example

```yaml
name: Tally key
slug: tallyKey
params:
  camera: { kind: target, interface: camera.ptz@1, required: true }
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
---
name: Camera strip
slug: cameraStrip
params:
  camera: { kind: target, interface: camera.ptz@1, required: true }
slots:
  - { id: tally, inputs: [momentary], outputs: [image] }
  - { id: rec,   inputs: [momentary], outputs: [image] }
parts:
  tally:
    template: <Tally key's ID>
    params:
      camera: { ref: { scope: params, path: [camera] } }
      label:  { expr: "target(params.camera).name" }
    slots: { key: tally }
bindings:
  rec:
    props:
      label: REC
      background: { use: rules, rules: [{ when: { expr: "target(params.camera).recording" }, value: "#d00" }], else: "#222" }
```

---

## 9. Flow templates

- **A flow with `params` is a template.** Its instances are flows with `template` and `params`, and their own `slug`, `enabled` and `owner`. Triggers, conditions, concurrency and steps come from the template.
- **`params` and `inputs` are different** (§2.1): params are bound once per instance and follow state, inputs are bound per run.
- **A flow instance with `forEach`** is one flow per item: `template: recorderWatchdog`, `forEach: {targets: {interface: "recorder@1"}}`, `params: {recorder: {expr: "item"}}` watches every recorder, including ones added during the show.
- **Each instance has its own runs** and its own concurrency. `flow.<slug>.items[<key>]:failed` is one recorder's watchdog failing, and `flow.<slug>.items.*:failed` (dataflow §2) is any of them.
- **A `call` step names a flow,** not an instance. Calling a flow template binds its `inputs`. Its `params` take their defaults, and calling an abstract one is `unboundParam`.

---

## 10. Open questions

1. ~~Should components nest?~~ Decided by [ADR 0032](../adr/0032-one-template-primitive.md): yes, as `parts`, with params bound in the outer component's scope.
2. ~~Should an instance be allowed to add bindings to controls its template leaves unbound?~~ Decided by [ADR 0032](../adr/0032-one-template-primitive.md): no. A layer stacked over it does that.
3. Do composites need their own run history and traffic view in the UI, or is the journal, filtered by `instance`, enough?
4. Should surfaces, control groups and variables become template kinds, or are layers, composites, components and flows enough until real projects ask?
5. Is 256 the right `forEach` limit, or should it follow the turn budget, as dataflow open question 4 asks for cell depth?
