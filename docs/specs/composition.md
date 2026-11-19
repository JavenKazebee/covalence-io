# Spec: Composition

- **Status:** Draft v0.3
- **Related:** [dataflow](dataflow.md), [surfaces](surfaces.md), [bindings](bindings.md), [flows](flows.md)

Users build bigger things out of smaller ones, and reuse them by giving them parameters, never by copying and editing. There is **one way to do that**: a **template** declares `params`, and an **instance** binds them (§1). It works the same for three kinds of entity:

| Kind | A template is | An instance is | Its body (§2) | Section |
|---|---|---|---|---|
| **Composite connection** | a connection built from others: "Camera" | a connection: "Camera A" | state, actions, events, owned flows | §6 |
| **Component** | anything on controls, with `params`: a "Tally key", a "Camera strip", a "Camera page" | a component shown on surfaces, or a **child** inside another component | controls, bindings, children, owned flows | §7, §8 |
| **Flow** | a flow with `params`: "Recorder watchdog" | a flow | steps | §9 |

**A component is whatever is on a set of controls,** at any size: one key, a strip of three, a whole page. It contains other components as children, so a page is made of strips and a strip of keys, and every level works the same way. Pages, shift layers and popovers are components **shown** on surfaces, with a rule for when (§7.3). Where a component sits in the nesting never decides when it's shown ([§11.8](#118-one-component-for-keys-strips-and-pages)).

**Instances take parameters, not overrides.** An instance can't change, add to or remove anything inside its template. Where copies may differ, the template says so with a param, which can hold a whole component or flow (§1.2). If one copy has to differ anywhere else, "Detach" turns it into a plain entity with no link back. That keeps "what does this do?" answerable from one place.

**Markers.** Everything here is **(M9)** ([M9](../milestones/m9-composition.md)) unless marked **(Later)**. Built so far, for flows (§9): params as cells and in scope (§1), flow params (§1.2), `forEach` (§4), the instance chain, its filters, validation and Detach (§5), and batches. Composites and components as templates, owned flows (§2), nesting (§3) and component params aren't yet. Components replace the `layer` and `control_binding` entities that M5 and M8 built (§7.4): the `component` entity, its embedded bindings and `show`, and the migration of stored projects are built. A component's children (§8) aren't yet, and the UI still edits a shown component as a layer and its bindings one by one.

---

## 1. Templates and instances

```ts
interface ParamDef extends StateSchemaNode {   // type, kind (a target with a constraint, a component, a flow…), description
  default?: Source<Value>;            // used by the template itself, and by instances that don't bind it
  required?: boolean;                 // no default and required: the template is abstract
}

interface TemplateFields {            // on a composite's CompositeDef, a component, a flow
  params?: { [name: string]: ParamDef };
}

interface InstanceFields {            // on a connection, a component, a child (§8), a flow
  template: string;                   // the template's ID
  params?: { [name: string]: Source<Value> };  // bound in the placer's scope (dataflow §8)
  forEach?: ForEach;                  // §4
}
```

- **A template is any entity of the three kinds that declares `params`.** An instance is an entity of the same kind with `template`, plus its own identity (`id`, `name`, `slug` or `alias`) and the fields that say where it appears:
  - a component instance: `show` (§7.3)
  - a child: the controls it takes in its container (§8)
  - a flow instance: `enabled`, `owner` (§2)
  - a composite instance: `enabled`
- **An entity is a template or an instance, never both.** A template can't derive from another template (`templateIsInstance`). It can contain instances (§3).
- **A template is usable itself where its kind can be** (a component with `show` is shown, a composite connects, a flow runs), with its params' defaults. One with a `required` param and no default is **abstract**: it can only be used through instances, and using it directly is `unboundParam`.
- **Params are sources** ([dataflow §1](dataflow.md)), evaluated reactively in the placer's scope, so an instance can follow state: `camera: {ref: {root: "vars", path: ["selectedCam"]}}`. A param's value is state at `<home>.params.<name>`, where `<home>` is the instance's home (§1.1).
- **Edits that span a template and its instances are saved together** as one batch, and cells keep their values through an edit when they still fit ([state spec §7.6](state-and-event-log.md#76-editing-a-running-show)).
- **Editing a template changes every instance at once.** A change an instance can no longer satisfy (a new required param, a type an existing binding doesn't match, a control that no longer fits) is refused with `badInstance`, listing the instances.
- **One module, `covalence_core::compose`,** binds, validates and evaluates instances of every kind. The UI shows one "instance of X, with these params" panel for all of them.

### 1.1 Homes

Each instance has a home in the state tree. Its params, cells and owned flows' status are under it.

| Kind | Home | Per item (§4) |
|---|---|---|
| Composite | `<alias>` | `<alias>.items[<key>]` |
| Component | `component.<slug>` | `component.<slug>.items[<key>]` |
| Flow | `flow.<slug>` | `flow.<slug>.items[<key>]` |
| A child (§8) | `<container's home>.parts.<slug>` | `<container's home>.parts.<slug>.items[<key>]` |

`component` replaces the `layer` root that M5 and M8 built (§7.4).

### 1.2 Params that hold a component or a flow
A template can declare where its instances may differ by more than a value: a param of `kind: "component"` or `kind: "flow"` holds a template, and the body uses it wherever it would name one.

```yaml
name: Camera strip
params:
  camera:    { kind: target, constraint: { template: <Camera's ID> }, required: true }
  recordKey: { kind: component, default: <Record key's ID> }
children:
  rec:
    template: { param: recordKey }
    params: { camera: { ref: { scope: params, path: [camera] } } }
    controls: { key: rec }
```

Cam C's strip binds `recordKey` to another component. It keeps every later fix to "Camera strip", and only its record key differs.

- **Where a body names a component or a flow, it can name a param instead:** `{param: <name>}` as a child's `template`, or a `call` step's `flow`.
- **The binding is an ID, given as a literal.** It doesn't follow state: `{expr: …}` or `{ref: …}` is `templateNotLiteral`. Changing which component a key is made of rebuilds its cells and flows, so it's an edit, not a value that moves during the show.
- **What's chosen must fit where it's used,** checked on save for every instance at every depth (`badInstance`):
  - a component: it has the controls the child maps, with the capabilities they need (`doesNotFit`); every param the use binds is one it declares, of a matching type (`unknownParam`, `badParam`); and its required params are all bound (`unboundParam`)
  - a flow: it has a `call` trigger, and the `call` step's inputs fit its `inputs`, as for any call (flows §8.3)
- **The instance chain names the template that was chosen** (§5), so `why` shows "Special record key, in Camera strip, camera = Cam C".
- **`recursiveTemplate` and `tooDeep` follow the bindings,** so a component can't reach itself through a param either.

---

## 2. Bodies and owned flows

- **A template's body** is the entity plus everything that belongs to it. A component embeds its bindings and children (§7, §8), and a composite its state, actions and events. Both own flows (by `owner`).
- **The body is evaluated once per instance,** with that instance's `params`. A component template's bindings and helpers have their cells under each instance: `<instance's home>.controls.<id>.<name>`.
- **Owned flows** are flows that belong to a component or a composite:

  ```ts
  interface Flow {                    // flows §1, with:
    owner?: string;                   // a component or composite ID
  }
  ```

  - In a project, they're `flow` entities with `owner`. A plugin's manifest component carries its flows inline, so a plugin can ship a component whole, and the core loads them as flows it owns.
  - They run **per instance** of their owner, children included. Their status is at `<owner's home>.flows.<slug>`, and their events at `<owner's home>.flows.<slug>:finished`.
  - Inside them, `params` is the owner instance's (dataflow §8).
  - Their event triggers name the owner's controls: `from: {controls: "key"}`, the owning component implied.
  - An event trigger on the owner matches events from every instance, and each run belongs to the instance the event came from.
- **Free-standing flows see only their own `params`.** A free-standing flow triggered by a control event gets `trigger.component` and `trigger.instance` (§5), not the component's params. A flow that should handle every instance of a component is owned by it.

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
  | Component | children: component instances on its controls, one or one per item (§8); flow instances it owns |
  | Composite | flow instances it owns. **(Later)** composite instances |
  | Flow | nothing; a flow uses others with `call` |

- **A nested instance binds its params in its container's scope.** `params` is always the innermost instance's own, so a value passes down one visible binding at a time: a "Camera strip" component's tally child gets `camera: {ref: {scope: "params", path: ["camera"]}}`, the strip's own camera.
- **No recursion.** A template that contains itself, directly or through others, can't be saved (`recursiveTemplate`). Instances nest at most 16 deep (`tooDeep`).
- **Editing a nested template** changes it everywhere it's used, at any depth, and the `badInstance` check covers every level.

### 3.1 Passing one thing, not many
- **Explicit scope means every level declares what it passes on.** A camera page that contains a camera strip that contains a tally key, where the key needs a switcher, an input and a recorder channel, has the page and the strip each declare and bind all three, though only the key uses them.
- **Bind a composite, not its parts.** Make "Cam B" a composite (§6) that holds the switcher, the input and the recorder, and pass one `camera` param down. Each level binds one value, and the key reads `target(params.camera).tally.program`. Adding an iris later changes the composite, not every level.
- **(Later)** If real projects still pass many loose values through several levels, a spread (`params: {...outer}`) would bind every param of the same name in one visible line. It isn't planned until a project needs it.

---

## 4. One instance per item

```ts
type ForEach = (
  | { items: Source<Value[]> }               // a live list: {expr: "target(vars.selectedCam).isos"}
  | { targets: Selector<Target> }            // every target the selector picks: {plugin: "io.covalence.recsim"}
) & {
  key?: Expr;                                // each item's identity; default the item, or a target's ID form
};
```

- **`forEach` makes one instance per item.** `item`, `index` and `key` are in scope while that instance's `params` are bound, and only there. Inside the body, the item is whatever param it was bound to: `params: {recorder: {expr: "item"}}`.
- **Per-item instances live at `<home>.items[<key>]`** (§1.1), so a reorder doesn't reset them, and other state can read them by key.
- **They follow the list.** An added item creates an instance, with its cells starting from `init`. A removed item ends one: its cells are dropped, and its owned flows' runs are cancelled with `removed`. Both are journaled, `instance.added` and `instance.removed` with `{instance}`, and `cause` → the patch or the domain change that changed the list.
- **They're never saved.** The entity is the one instance with `forEach`. Its items come from state each time it's loaded.
- **Each key once.** An item whose key repeats an earlier one's isn't instanced.
- **At most 256 items.** Extra items aren't instanced, and the instance reports `tooMany`: how many there are is state at `<home>.tooMany`, gone again once the list fits. The cap guards the turn budget (dataflow §5) against a list bound to the wrong thing, such as a media server's 10,000 clips; no real rig is near it.
- **Per kind:**
  - **A composite with `forEach`** makes each item a target (§6.4).
  - **A shown component with `forEach`** is one shown component per item, each navigated by `{component, item: key}` ([core actions §3.1](core-actions.md)): a page per camera.
  - **A child with `forEach`** is one child per item, each on the next control of its `over` selector: a key per camera, a grid of ISO values (§8.2).
  - **A flow with `forEach`** is one flow per item, each with its own runs and concurrency: a watchdog per recorder.
- **`forEach` instances nest.** Inside a per-item instance, a nested `forEach` sees its own `item`, not the outer one's. The outer item reaches it through a param.

---

## 5. Identity, validation and Detach

- **Every journal entry made inside an instance carries `instance`:**

  ```ts
  type InstanceChain = { template: string; instance: string; key?: PathSegment }[];   // outermost first
  ```

  Runs, steps, actions, cells' patches and events made inside an instance all carry it, and an event's `trigger.instance` is it. It replaces the separate `placement` field and the `<component>/<flow>` run identity planned before it.
- **The chain holds IDs, never names or values,** so a rename doesn't rewrite history and every entry stays small. The server resolves names when it answers: each link of an entry from `GET /journal` and `why` has `name` and `templateName` from the entities as they are now. **(M9)** One since deleted is named from the op log.
- **A run records the params it started with:** `run.started` has `params`, the owner instance's bound values at that moment (state spec §2.2). Everything a run does points back to its start, so `why` on an action shows "Tally key, camera = Cam B, in Cameras[camB]", even after the param has moved on and past the memory window. An entry outside a run (a cell's patch, a control event) shows its instances, and their param values only while the patches that set them are still in memory.
- **Filtering:** `GET /journal` and `GET /runs` take `instance=<id>`, which matches an entry with that instance anywhere in its chain, so "Cameras" gives every camera and "Cam B" gives everything inside it; `key=<key>` narrows to one item of a `forEach` instance; and `template=<id>` matches any instance of that template, at any depth. The timeline and the runs list offer both.
- **A composite's history is the journal filtered by its instance.** It has no run history or traffic view of its own; its actions' runs and the real devices' traffic are already there.
- **Validation** (errors block saving):
  - `badInstance`: a required param unbound, a binding whose type doesn't match the `ParamDef`, a child whose controls don't fit its template's. When a template is saved, every instance of it at every depth is checked, and the error lists the ones that fail.
  - `unknownTemplate`, `templateIsInstance`, `recursiveTemplate`, `tooDeep`.
  - `unboundParam`: an abstract template used directly.
  - `tooMany` is reported at run time, not at save.
- **Detach** works the same for every kind. It turns an instance into a plain entity with its template's body copied in and its params left as they were bound, as sources: the template's params become the entity's own, each defaulting to the instance's binding, so the body reads `params` as before. A `forEach` instance detaches to one plain entity per current item, with each item's value written in as a literal (an expression that reads `item` is wrapped in a function that's given it). The editor warns that they won't follow the list anymore. Detach is one-way: for a flow, `POST /flows/{id}/detach` answers `{flows}`.
- **Detaching a child** copies its template into a new plain component, with no link back, and points the child at the copy, which nothing else uses. The child keeps its params and its controls.
- **A template with instances can't be deleted** (`409`, naming them). Detach or delete them first.

---

## 6. Composite connections

A **composite** is a connection whose definition is a project entity, not a plugin. It has an alias, state, actions and events like any other connection, so nothing downstream knows it's composite. A **composite template** is also the project's own contract for a kind of thing: a component, binding or flow constrained to it works with every instance, whatever devices each one binds (§6.5).

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

### 6.4 Composites per item
- **Each item of a composite with `forEach` is a target,** with its state at `<alias>.items[<key>]`. A target is a connection with nothing within it (plugin protocol §13.9), so each item is a connection of its own:
  - **Its ID is derived:** `<instance ID>[<key>]`, the key in the state path's text form (state spec §1.1). It's the same for as long as the item's key is, and across restarts, so a variable or a journal entry that holds it keeps meaning the same camera.
  - **Its alias is its home,** `cams.items["Cam A"]`, which is what the editor shows and what `target(t).alias` reads.
  - **It's listed with the connections,** in `GET /connections`, with `item: {instance, key}`. It can't be edited, disabled or deleted itself: change the instance, or the list.
- **Each item is an instance of the template,** so `{template}` selects each item as it does a single instance (§6.5).
- **Actions and events are per item:** an action on an item's target runs with that item's `params`, and an item's events carry its key in `instance`.

### 6.5 Templates as contracts
- **A constraint `{template: <id>}` accepts every instance of a composite template,** and each item of a `forEach` one, and nothing else ([plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed)). The editor types parameters and `target(t)` from the template's actions and state, which every instance shares.
- **Different devices behind one template are bridged by the template, in the open.** Instances bind whatever devices they have; where plugins differ, the template's state expressions and owned flows handle it, for example by branching on `target(params.ptz).plugin`. `why` follows the branch to the real device.
- **Every action is declared.** A composite offers only the actions it lists, each a `forward` or an owned flow. Nothing is passed through wholesale, so the template's shape is the same whatever an instance binds.

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
    ptz:      { kind: target, constraint: { plugin: io.covalence.ptzsim }, required: true }
    switcher: { kind: target, constraint: { plugin: io.covalence.switchersim }, required: true }
    input:    { type: integer, required: true }
    recorder: { kind: target, constraint: { plugin: io.covalence.recsim } }
  state:
    tally:
      program: { expr: "target(params.switcher).program == params.input" }
      preview: { expr: "target(params.switcher).preview == params.input" }
    recording: { expr: "target(params.recorder).recording" }
  actions:
    recallPreset:
      params: { type: object, properties: { preset: { type: integer } }, required: [preset] }
      forward:
        target: { ref: { scope: params, path: [ptz] } }
        action: recallPreset
        params: { preset: { expr: "input.preset" } }
    takeLive:
      params: { type: object }
      flow: 01JC0TAKE0LIVE0FLOW00000000   # a flow with owner: this composite
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

`cams.items["Cam A"].tally.program` follows the switcher, and the "Cam A" item, as a target, is accepted by `{template: <Camera's ID>}`. A recorder that isn't set reads as disconnected. A rig with a second make of PTZ widens `ptz` to an unconstrained target and makes `recallPreset` an owned flow that branches on `target(params.ptz).plugin`.

---

## 7. Components

A **component** is whatever is on a set of controls: what they look like, how their input is shaped, and what they do, with parameters. One key, a strip of three keys and a whole page are all components, and bigger ones are made of smaller ones (§8). A component **shown** on surfaces is a page, a shift layer or a popover, by its `show` (§7.3). Plugins ship components as content with no code ([plugin protocol §13.6](plugin-protocol.md#136-device-drivers-are-plugins-the-core-performs-hardware-io)), replacing manifest `presets`.

```ts
interface Component extends TemplateFields {   // project entity `component`, or manifest `components`
  id: string;
  name: string;
  slug: string;                       // home: component.<slug> (§1.1)
  controls: ControlSpec[];            // the controls it covers: a page's keys, or a key's one control
  bindings?: ControlBinding[];        // bindings §1: a control's look, helpers, gestures and input
  children?: { [slug: string]: Child };   // §8: components on some of its controls
  show?: Show;                        // §7.3: shown on surfaces; absent, it's only used inside others
  // as an instance (§1): template, params, and with `show`, forEach
}

interface ControlSpec {               // was a layer's LayerControl and a component's slot
  id: string;                         // shown: matches a ControlDef.id on the surfaces it fits; used: a name, "key"
  at?: Placement;                     // where the editor draws it
  inputs?: InputKind[];               // capabilities it needs the control to have
  outputs?: OutputKind[];
}
```

### 7.1 Controls and bindings
- **A component lists the controls it covers,** at most 256, with unique IDs (`duplicateControl`). A shown component's control IDs are the surface's, and it fits a surface that has them all with the capabilities listed (surfaces §5.4). A component used as a child names its own ("key", "tally"), and its container maps them onto its controls (§8).
- **Each control is covered once:** by one binding, or by one child (`duplicateControl`). A control with neither is blank and ignores input.
- **Bindings are embedded** in their component, each with its own `id` and `control` (bindings §1), so a helper's cell keeps its identity when the binding moves to another control (dataflow §5.4). A binding's properties must be outputs its control declares (`noOutput`).
- **Helpers are cells under the component's home:** `<home>.controls.<control>.<name>`, so each instance has its own. With `scope: "surface"` they're at the surface instead (surfaces §4.3).
- **The key editor shows the chain** for a key inside a child: "Tally key, camera = Cam B, in Camera strip, in Cameras". That answers "what does this key do?" from one place. "Save as component" turns a key's binding into a new one-control component and puts a child in its place.

### 7.2 Owned flows and events
- **Owned flows run per instance,** children included (§2). Their event triggers name the component's own controls: `from: {controls: "key"}`.
- **An event on a control carries `component`,** the shown component that controlled it when the input arrived (surfaces §5.2), and `instance`, the chain down to the child whose control it is (§5). An event trigger `{component, controls?}` matches input on that component's controls wherever it's shown or used, so one trigger on "Tally key" covers every tally key on every page.
- **An instance can't add bindings** to controls its template leaves blank. A component shown over it does that (surfaces §5).

### 7.3 Showing a component

```ts
interface Show {                      // surfaces §5
  on?: Selector<SurfaceId>;           // surfaces or surface groups it may appear on
  when?: When;                        // default { nav: "main" }
  level?: "under" | "normal" | "over";   // default "normal"
  rank?: Source<number>;              // within the level; default 0
  cascade?: "replace" | "through";    // default "replace"
}
```

- **Only components with `show` are candidates** in resolution (surfaces §5.2): for each surface and control, the shown component that wins controls it, sets its look and gets its input. That's the rule M8 built for layers, unchanged.
- **A child is on wherever its container is.** Children have no `show`. Whether a key is visible is decided once, by the shown component it's in, never by where it sits in the nesting ([§11.8](#118-one-component-for-keys-strips-and-pages)).
- **`show` belongs to the entity, never to its template.** A template with `show` is shown itself, with its params' defaults, and each instance with `show` is shown as it says. A component used as a child is used, whatever `show` it has itself.
- **A component instance entity is shown.** Children are written inside their container (§8), so a component with `template` and no `show` would appear nowhere, and is `invalid`.
- **With `forEach`,** a shown instance is one shown component per item, its `show` evaluated per item with `item` in scope, and navigated by `{component, item: key}` (§4).
- **The editor names shown components by their preset** (surfaces §5.3): a page, a shift layer, a popover, an overlay, a section. "Layer" survives only in those names.

### 7.4 What components replace

Components replace the `layer` and `control_binding` entities that M5 and M8 built, and the `placement`, part, `control_group` and layer template that M9 planned. M9 migrates stored projects when it loads them:

| Was | Now |
|---|---|
| A `layer` | A component with `show`: `on`, `when`, `level`, `rank` and `cascade` move into it unchanged, and its `controls` stay its controls |
| A `control_binding` (`layer`, `control`) | A binding in that component's `bindings`, with its `id` and `control`, so its helpers keep their cells |
| A layer template and its instances | A component template with `show`, and instances with `show` |
| A `placement` on a layer's controls, a component's `parts` | A child (§8) |
| A component's `slots` | Its `controls` |
| A helper's `scope: "layer"` | `scope: "component"` |
| A `control_group` | A child with `forEach` and `over` (§8.2) |
| The `layer` state root, `layer.<slug>…` | `component.<slug>…` |
| `surface.<slug>.layers`, `.controls.<id>.layer`, `.look` | `surface.<slug>.shown`, `.controls.<id>.component`, `.look` naming components |
| `startLayer` on a surface | `startComponent` |
| `{layer, controls}`, `{slot}`, `{group}` event sources; `trigger.layer` | `{component, controls}`; `trigger.component` |
| `navigate {layer}`, `POST /surfaces/{id}/current {layer}` | `{component}` |
| `/layers`, `/control-bindings` | `/components` (surfaces §9) |

- **Refs, event sources and `navigate` params are rewritten** by shape. Expressions that read the `layer` root, `trigger.layer` or a resolution result are rewritten where they name it by a dotted name, as the `resolutionCycle` check reads them (surfaces §5.2); a name built at run time isn't found, and each expression rewritten is logged as a warning so it can be checked.
- **The migration runs when the server starts,** before any entity is loaded, and writes every rewritten entity in one transaction, so a project is migrated as a whole or not at all. It's idempotent, and old journal entries are read with `layer` as `component`.

---

## 8. Children

A **child** is a component instance inside another component, on some of its controls. A key on a page, a strip on a page and a key in a strip are all children.

```ts
type Child = InstanceFields & { slug: string } & (    // template: a component's ID, or {param} (§1.2)
  | { controls: { [its: string]: string } }          // its controls → this component's controls
  | { forEach: ForEach;                              // §8.2: one child per item
      over: Selector<ControlId>;                     // this component's controls, one per item in selector order
      overflow?: "clip" | "scroll" }
);
```

- **A child takes the controls it's given.** Each of its template's controls maps to one of the container's, with at least the capabilities the template lists (`badInstance`, with `doesNotFit`); every one is mapped (`unmappedControl`). Placing a strip places everything inside it.
- **Params are bound in the container's scope** (dataflow §8), so a child on a camera page passes the page's camera on: `camera: {ref: {scope: "params", path: ["camera"]}}`.
- **A child's home** is `<container's home>.parts.<slug>`: its params, its helpers' cells under `controls`, its children, and its owned flows' status.
- **Children nest** at most 16 deep (§3), and a component can't contain itself (`recursiveTemplate`).
- **A child has no `show`** (§7.3), and no `when` of its own. **(Later)** If real decks need part of a page to change without a separate shown component, a child `when` would be the place, as an open question (§10, question 8).

### 8.1 Example

```yaml
name: Tally key
slug: tallyKey
params:
  camera: { kind: target, constraint: { template: <Camera's ID> }, required: true }
  label:  { type: string }
controls:
  - { id: key, inputs: [momentary], outputs: [image] }
bindings:
  - id: look
    control: key
    props:
      label: { ref: { scope: params, path: [label] } }
      background:
        use: rules
        rules:
          - { when: { expr: "target(params.camera).tally.program" }, value: "#d00" }
          - { when: { expr: "target(params.camera).tally.preview" }, value: "#0a0" }
        else: "#222"
# owned flow, a flow entity with owner: Tally key
#   triggers: [{ kind: event, from: { controls: key }, event: press }]
#   body: [{ id: pick, kind: set, path: vars.selectedCam, value: params.camera }]
---
name: Camera strip
slug: cameraStrip
params:
  camera: { kind: target, constraint: { template: <Camera's ID> }, required: true }
controls:
  - { id: tally, inputs: [momentary], outputs: [image] }
  - { id: rec,   inputs: [momentary], outputs: [image] }
children:
  tally:
    template: <Tally key's ID>
    params:
      camera: { ref: { scope: params, path: [camera] } }
      label:  { expr: "target(params.camera).name" }
    controls: { key: tally }
bindings:
  - id: rec
    control: rec
    props:
      label: REC
      background: { use: rules, rules: [{ when: { expr: "target(params.camera).recording" }, value: "#d00" }], else: "#222" }
---
name: Cameras
slug: cameras
controls: [ k0, k1, … k14 ]                # the deck's keys, as ControlSpecs
show: { when: { nav: main } }              # a page
children:
  camA: { template: <Camera strip's ID>, params: { camera: <Cam A> }, controls: { tally: k0, rec: k5 } }
  camB: { template: <Camera strip's ID>, params: { camera: <Cam B> }, controls: { tally: k1, rec: k6 } }
bindings:
  - { id: back, control: k14, props: { label: Back } }
```

### 8.2 One child per item

A child with `forEach` spreads a live list over the container's controls: a key per camera, a grid of ISO values for the selected camera, a row of router inputs. It replaces control groups.

```yaml
children:
  isos:
    template: <ISO key's ID>                       # a component with one control
    forEach: { items: { expr: "target(vars.selectedCam).iso.options" } }
    over: { region: { x: 0, y: 1, w: 5, h: 2 } }
    params: { value: { expr: "item" } }
    overflow: scroll
```

- **Each item takes the next control of `over`,** in selector order (dataflow §2). The template has exactly one control (`badInstance`); items beyond the controls are clipped, or scrolled with `overflow: "scroll"`. Controls with no item are blank.
- **The child owns every control of `over`.** A binding or another child on one of them is `duplicateControl`.
- **Each item is an instance** at `<container's home>.parts.<slug>.items[<key>]`, created and ended with the list (§4). Nothing is saved per item.
- **Triggers see the item.** An event from one of its controls carries `item`, `index` and `key` in `trigger`, so one flow owned by the template, or one trigger on the container's controls, handles the whole grid.
- **`overflow: "scroll"`** keeps the offset at `<container's home>.parts.<slug>.offset`. Flows or keys outside the child move it.

---

## 9. Flow templates

- **A flow with `params` is a template.** Its instances are flows with `template` and `params`, and their own `slug`, `enabled` and `owner`. Triggers, conditions, concurrency, inputs, outputs and steps come from the template; an instance that has any of its own is `invalid`. A template with no required param runs by itself too, with its defaults; an abstract one only runs through its instances.
- **An instance's params are cells** at `flow.<slug>.params.<name>`, and its steps, triggers and conditions read them as `params`. `run.started` records them.
- **`params` and `inputs` are different** (§2.1): params are bound once per instance and follow state, inputs are bound per run.
- **A flow instance with `forEach`** is one flow per item: `template: recorderWatchdog`, `forEach: {targets: {plugin: "io.covalence.recsim"}}`, `params: {recorder: {ref: {scope: "item", path: []}}}` watches every recorder, including ones added during the show. A target item is `{conn}`, keyed by the connection's ID. Each item runs as the flow `<instance ID>[<key>]` (the key as in a state path's brackets), at `flow.<slug>.items[<key>]`: `POST /flows/{that}/run` starts it.
- **Each instance has its own runs** and its own concurrency. `flow.<slug>.items[<key>]:failed` is one recorder's watchdog failing, and `flow.<slug>.items.*:failed` (dataflow §2) is any of them.
- **A `call` step names a flow,** or `{param}` for the flow a `flow` param holds (§1.2). Calling a flow template binds its `inputs`. Its `params` take their defaults, and calling an abstract one is `unboundParam`. A `forEach` instance is a flow per item, so a call can't name it (`notCallable`).

---

## 10. Open questions

1. ~~Should components nest?~~ Decided: yes, as children, with params bound in the outer component's scope (§8, §11.8).
2. ~~Should an instance be allowed to add bindings to controls its template leaves unbound?~~ Decided: no. A component shown over it does that.
3. ~~Do composites need their own run history and traffic view in the UI, or is the journal, filtered by `instance`, enough?~~ Decided: the journal filtered by `instance` (§5).
4. Should surfaces, variables and sources become template kinds? A **source template** (a `Source` with `params`) would let a configured chain, such as "audio taper with a dead zone" or "seconds remaining → `MM:SS`, amber under 60 s", be named and reused without writing a project transform in JS. Or are composites, components and flows enough until real projects ask?
5. ~~Is 256 the right `forEach` limit, or should it follow the turn budget, as dataflow open question 4 asks for cell depth?~~ Decided: a fixed 256 (§11.3).
6. ~~**Params that hold a template.** Should a `ParamDef` be able to hold a component, a flow or a source, so a template declares where it can vary?~~ Decided: a component or a flow (§1.2). A source waits on source templates (question 4).
7. ~~**How a `forEach` composite's items are targets** (§6.4).~~ Decided: each is a connection with an ID derived from the instance and the key, its home as its alias, listed with the connections (§6.4, §11.4).
8. **Children that come and go.** Part of a page that changes while the rest stays (a folder in one region, Bitfocus Buttons' sections) is a separate shown component today: a section preset (surfaces §5.3). Should a child have a `when` instead? It would make visibility depend on nesting again, which §11.8 rejects, so it waits for a real deck that a shown component handles badly.

---

## 11. Decisions

Why composition works this way, and what was rejected. Keep these notes when the sections above change.

### 11.1 Reuse is by parameters, never by copying or overriding
- **Decided:** templates declare `params`, instances bind them (§1), and instances can't change, add to or remove anything in their template. Detach is the way out. `params` is always the own container's: nothing is inherited, and a value passes down one visible binding at a time, like arguments into a function call (§3).
- **Why:** "Create from template" stamped out copies that drifted: a fix to one camera's layer didn't reach the others, and nothing recorded that they were ever the same. What a key did was spread across a binding, flows and the layer's parameters. The goal is composition over inheritance, so "what does this do?" and "where does this value come from?" are answered in one hop per level. It costs more typing than inheritance; the editor fills the common case.
- **Rejected:**
  - *Copies:* drift is guaranteed, and "these are all the same" isn't recorded anywhere.
  - *Instances with overrides:* what most tools do, but every instance becomes a partial copy, and finding a value means walking a chain.
  - *Lexical inheritance of `params`* (down the layer tree, or into nested templates): values arrive from places the user can't see, and moving a layer changes its meaning.
  - *Templates derived from templates:* each level becomes a partial override, and the chain comes back.
  - *Components as plain flows and bindings with a naming convention:* nothing ties them together, so neither the editor nor a plugin can treat them as one thing.

### 11.2 One template primitive for three kinds
- **Decided:** composites, components and flows are all templates the same way, and one module binds, validates and evaluates instances of every kind, with one identity in the journal (`instance` chains), one `badInstance`, one Detach and one UI panel. Owned flows run per instance with its params, and free-standing flows see only their own. Flows keep `inputs` beside `params`: params are bound where an instance is placed and follow state; inputs are bound per invocation, and a run isn't an instance. There were four kinds until layers became components (§11.8).
- **Why:** M9 had planned four mechanisms (layer templates, component placements, composite templates, flows with inputs) that would each build their own binding, scope, evaluation, identity, validation and Detach. That's the problem sources and selectors fixed for values (dataflow §10.1): the copies drift, and users learn four editors for one idea. Built once, nesting and per-item instances work for every kind, and a new kind only needs its body defined.
- **Rejected:**
  - *Four mechanisms:* each simpler alone, every later improvement built four times.
  - *A single `template` entity with kind-specific payloads:* each kind has its own store, API and editor, and an instance must be its kind's entity anyway to appear where that kind does.
  - *Merging flow `inputs` into `params`:* every call would create an instance with a state home and a lifetime, for something that already has a run.

### 11.3 One instance per item of a live list
- **Decided:** `forEach` makes one instance per item of a list or target selector (§4), keyed so a reorder doesn't reset them, created and removed with the list and journaled with `cause`, at most 256. Each item of a composite with `forEach` is a target. A child with `forEach` spreads its items over its container's controls (§8.2), which replaces control groups.
- **Why:** lists that change while the show runs were handled in one place only (control groups). A composite per camera, a page per camera or a watchdog flow per recorder would otherwise need a copy per item. Per-item instances are never saved: the entity is the one `forEach` instance and its items come from state, so the core still never edits the project.
- **Rejected:** per-item copies made by the editor; copies don't follow the list while the show runs.
- **The cap is a fixed 256.** It's there so a list bound to the wrong thing (10,000 clips rather than 6 cameras) reports `tooMany` and the show keeps running, rather than every turn stalling while thousands of instances are built.
  - *A cap set per instance:* a setting nobody needs until a real rig is over 256, and it can be added then.
  - *A cap from the turn budget:* stopping where the budget runs out would make the same project build a different number of items on a Pi and on a Mac.

### 11.4 Composite connections
- **Decided:** a composite is a connection (plugin `core.composite`) defined in the project, with params, state as cells, actions that run a flow or forward to another target, and re-emitted events (§6). A composite template is the contract that a constraint names (§6.5). Nothing downstream knows it's composite. Causation runs through it, so `why` reaches the real device, and its actions are `internal` while the actions they cause are gated themselves.
- **Why:** a "camera" on a show is several devices: PTZ from one connection, tally from the switcher, record state from a recorder, an iris from a CCU. A composite makes "Camera A" one target. Its template gives every camera the same shape, so one component or flow works with all of them, and devices from different plugins are bridged in the template, where users can see and change it, without writing a plugin. A composite's state is only as fresh as what it reads.
- **Rejected:**
  - *Aggregator plugins:* hand-written per combination, and the journal sees only the aggregator.
  - *Device interfaces that composites implement, with `delegate` to pass a whole interface through:* dropped with device interfaces themselves ([plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed)). Without them, a template declares each action it offers, which is more typing for a composite that only passes a PTZ through, and keeps its shape the same whatever an instance binds.
  - *Standalone interface implementations* (an entity mapping a connection onto an interface): moot without device interfaces; a composite does the mapping.
  - *Targets spanning several connections* (`{conn: [a, b]}`): every place that resolves a target would merge state and route actions.
  - *Component parameters only:* every component, binding and flow repeats the wiring, and nothing can select "a camera" as one thing.
- **Each item of a `forEach` composite is a connection** (§6.4), with an ID derived from the instance and the key, its home as its alias, and a read-only entry in the connections list. Targets stay `{conn}` everywhere, so the gate, the journal, constraints and the editor don't learn a second kind.
  - *Items as `{conn, key}` targets:* the shape plugin protocol §13.9 rejected for plugins, for the same reasons.
  - *A random ID per item:* it would change on every restart, since items are never saved (§4), so a stored target would stop pointing at its camera.

### 11.5 Params that hold a component or a flow
- **Decided:** a param can be `kind: "component"` or `kind: "flow"`, bound to a template's ID as a literal, and a body names it with `{param}` wherever it would name a component or a flow (§1.2). What's chosen is checked against where it's used, for every instance, on save.
- **Why:** reuse by params left Detach as the only way for one copy to differ, and Detach cuts it off from every later fix (§11.1). One camera with a different recorder shouldn't cost that. The template's author declares which parts may be swapped, so "what does this do?" is still answered by the template and the instance's params.
- **Rejected:**
  - *Overriding a part in one instance:* the overrides §11.1 rejects, one part at a time.
  - *Bindings that follow state:* a key would change what it's made of mid-show, rebuilding its cells and flows on a value change, and it couldn't be checked on save.
  - *Source params:* a source isn't a template yet (§10, question 4).
  - *A second template for the copy that differs:* two templates to keep in step by hand, which is the drift §11.1 exists to stop.

### 11.6 Deleting a template
- **Decided:** a template with instances can't be deleted; the answer names them (§5).
- **Why:** an instance without its template has no body, so it would stop doing anything, and the show would find out when a camera stopped working.
- **Rejected:**
  - *Deleting the instances with it,* as a layer takes its bindings: one delete would take out every camera.
  - *Detaching them automatically:* copies appear that nobody asked for, and nothing says they were ever the same.

### 11.7 The instance chain in the journal
- **Decided:** entries carry the chain as IDs; `run.started` records the params a run started with; the journal and the runs list filter by instance (at any depth, and by item) and by template; the server resolves names when it answers (§5).
- **Why:** "why did this fire?" has to say which camera it was, and params can follow state, so their value now isn't their value then. A run is where actions come from and it's kept on disk, so recording params once per run answers the question that matters at the cost of one field.
- **Rejected:**
  - *IDs only, with params read back from state history:* params are cells, kept only in memory (state spec §2.4), so on a Pi the answer would be gone within the hour.
  - *Params on every entry made inside an instance:* every cell patch and event made heavier, for entries whose question is rarely "with which params?".
  - *Names in the chain:* a rename would leave history naming something that no longer exists, and every entry grows.

### 11.8 One component for keys, strips and pages
- **Decided:** a component is whatever is on a set of controls, at any size, and contains other components as children (§7, §8). It replaces layers, control bindings as entities, placements, parts, control groups and layer templates (§7.4). **Containment recurses; activation doesn't.** Only components with `show` take part in resolution, by the flat rule of surfaces §5.2 (`when`, level, rank, recency); a child is on wherever its container is, and has no `when`. Bindings are embedded in their component, each keeping an `id`. A child with `forEach` spreads a list over its container's controls. Composites stay their own kind. The `layer` state root becomes `component`.
- **Why:** putting something on keys took seven nouns, each with its own fields, rules and editor: a binding for one key, a component for a reusable key, a placement to put it on a layer, a part to put it in another component, a layer for a page, a layer template to reuse a page, and a control group to fill keys from a list. They're one idea at different sizes. With one, users learn "a component covers controls, can contain components, takes params, and can repeat", and pages, shift layers and popovers are presets of when it's shown. Only layers and control bindings were built, so the change costs one migration.
- **Rejected:**
  - *Keeping the seven:* every improvement (a new capability check, a scroll mode, `why` through nesting) built for each, and a key editor that changes shape with what the key happens to be.
  - *Activation by position in the tree* (a child `when`, or switch nodes as in SKAARHOJ Reactor): surfaces §11.3 rejected it, and the reasons hold. Moving a branch in the editor would change the show, and "the last mode turned on wins" or a strip under every page don't fit a tree. The tree preset (surfaces §5.3) stays a way to write `when`, not a structure resolution walks.
  - *Inline component bodies in children:* a second way to write a key beside a binding. A repeated child names a component; the editor makes one from a key in one step.
  - *Bindings and children as their own entities pointing at their container,* as `control_binding` and `placement` were: two shapes for the same thing, a plugin's component and a project's page stored differently, and one key's meaning spread across rows.
  - *Composites as components with no controls:* a composite is a device you build, targeted by actions and constraints; a component is what's on keys. Merging them would make every component a possible target.
  - *Keeping the `layer` root and naming:* two names for one thing in every expression, path and API, for the sake of a migration that's needed anyway.
