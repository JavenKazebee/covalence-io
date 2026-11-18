# Milestone 9: Composition

- **Status:** Planned, after [M8](m8-dataflow.md)

Users build bigger things out of smaller ones, and reuse them by parameters, never by copying ([composition §11.4](../specs/composition.md#114-composite-connections), [composition §11.1](../specs/composition.md#111-reuse-is-by-parameters-never-by-copying-or-overriding), [composition §11.2](../specs/composition.md#112-one-template-primitive-for-four-kinds)):
- **One template primitive.** Layers, composite connections, components and flows declare `params`, and instances bind them. Binding, scope, identity, validation and Detach are built once, for all four.
- **Composite connections** make "Camera A" one target out of a PTZ head, a switcher's tally and a recorder, and a composite template lets one layer control cameras from different plugins.
- **Templates nest,** so a camera strip is made of tally keys, and **`forEach`** makes one instance per item of a live list: a composite per camera, a watchdog flow per recorder.
- **Components** package a control's look, helpers and flows with parameters. Plugins ship them in place of presets.
- **Transforms from plugins and projects** run in the core's sandbox beside the built-ins ([dataflow §10.3](../specs/dataflow.md#103-sources-transforms-and-sinks-helpers-are-transforms)).

## Done when

- [ ] A composite "Camera A" over a simulated PTZ camera, a simulated switcher and a simulated recorder: its `tally.program` follows the switcher, `recallPreset` forwards to the PTZ, and `takeLive` runs a flow it owns. `why` on the PTZ's `action.result` walks back through the flow's run to the composite's `action.requested`.
- [ ] A step constrained to `{template: <Camera>}` accepts "Camera A" and refuses the bare PTZ. An event trigger on `{target: {template: <Camera>}}` fires for every instance.
- [ ] Two simulated PTZ plugins with different preset actions behind one "Camera" template: its `recallPreset` is an owned flow that branches on `target(params.ptz).plugin`, and one layer recalls presets on both. A step constrained to `{plugin: …}` works on a selected camera of that plugin with no composite.
- [ ] A composite template "Camera" with three instances, one per camera. Editing the template changes all three, and a change that breaks an instance's params is refused, naming it.
- [ ] One "Cameras" composite instance with `forEach` over a list in `vars`. Each item is a target at `cams.items[<key>]`, accepted by `{template: <Camera>}`. Adding an item to the list adds a camera, journaled `instance.added`; removing one ends it, and its owned flow's active run is cancelled with `removed`.
- [ ] A layer template "Camera control" with a `camera` param, instanced per camera. One flow owned by the template handles a key on every instance with that instance's `params.camera`. A free-standing flow on the same key gets `trigger.instance` but no `params`.
- [ ] A "Tally key" component, placed four times on a layer with a different camera each, draws each key's tally, and its owned flow selects that key's camera. The key editor shows "Tally key, camera = Cam B".
- [ ] A "Camera strip" component with a "Tally key" part, placed on a layer template's instance. The part's tally follows the strip's camera, which follows the layer instance's. `why` on its owned flow's action shows the chain: layer instance, placement, part. A component that contains itself can't be saved (`recursiveTemplate`).
- [ ] A control group places the "Tally key" component per camera in a live list.
- [ ] A flow template "Recorder watchdog" with a `recorder` param, instanced with `forEach: {targets: {plugin: <recorder sim>}}`, runs once per recorder, including one connected during the test. A `call` to a flow with `inputs` still binds them per run.
- [ ] A test plugin ships a component and a transform. The component can be placed; the transform runs in the core (it still works with the plugin host stopped) and is named `<plugin id>/<id>`.
- [ ] A project transform (`project/<slug>`) is defined in the UI and used as a helper.
- [ ] "Detach" turns an instance or a placement of each kind into plain entities with no link back, and a `forEach` instance into one entity per current item.

## Phases

### Phase 0: Decisions and docs
- [x] Composites and reuse by parameters drafted in the [composition spec](../specs/composition.md) (§11.1, §11.4), and scope in [dataflow §8](../specs/dataflow.md).
- [x] One template primitive ([composition §11.2](../specs/composition.md#112-one-template-primitive-for-four-kinds)): owned flows, nesting, `forEach`, the `instance` chain. Composition spec v0.2.
- [x] Interfaces dropped for now, the core's included (they only named its collections), and composite templates are how devices from different plugins are controlled as one kind ([plugin protocol §13.9](../specs/plugin-protocol.md#139-dynamic-targets-proposed), [composition §6.5](../specs/composition.md#65-templates-as-contracts)).
- [ ] Dynamic targets for plugins ([plugin protocol §13.9](../specs/plugin-protocol.md#139-dynamic-targets-proposed)): `{conn, key}` items of a plugin's collections, plugin constraints on steps and state, and `target(t)` with `.plugin` and `.supports`. Today only `core`'s collections are targets, so these come first in this milestone.
- [ ] **Changesets** ([state spec §7.6](../specs/state-and-event-log.md#76-editing-a-running-show-proposed-kept-state-settled-in-m8)): drafts across entities, published as one operation and validated as a whole; runs record the revision they started on.
- [ ] The `instance` chain's API shape: how the timeline, `why` and the runs list show and filter it.
- [ ] Accept the composition design ([composition §11](../specs/composition.md#11-decisions)) and live edits ([state spec §7.6](../specs/state-and-event-log.md#76-editing-a-running-show-proposed-kept-state-settled-in-m8)): drop "proposed" from their headings.

### Phase 1: Flow inputs and outputs (Rust)
- [ ] `inputs` (as `ParamDef`s) and `outputs` on flows, `stop` with outputs, and the `call` step with `wait`. Composite actions need them.

### Phase 2: The template primitive (Rust)
- [ ] `covalence_core::compose`: `ParamDef` (source defaults, `required`), `template` and `params` on an instance, binding in the placer's scope, homes (composition §1.1), abstract templates, `badInstance` checked across every instance at every depth.
- [ ] `params` in scope (dataflow §8) and `{scope}` refs. The `layer.params` shorthand is rejected.
- [ ] Owned flows: `owner`, per-instance runs, `params` from the owner instance, and the owner's controls or slots in triggers.
- [ ] Nesting: instances inside a template's body, `recursiveTemplate`, `tooDeep`.
- [ ] `forEach`: per-item instances at `<home>.items[<key>]`, created and ended with the list, `instance.added` and `instance.removed`, cancellation with `removed`, `tooMany`.
- [ ] The `instance` chain on journal entries and `trigger.instance`.
- [ ] "Detach", for every kind.

### Phase 3: Template kinds (Rust)
- [ ] **Composites:** plugin `core.composite`: `params`, state cells, `connected`, `forward` and owned-flow actions, re-emitted events, `{template}` constraints and selectors. Causation through composite actions and events; `cycle`, the loop guard and `tooDeep`. Collections through `forEach`, each item accepted by `{template}`.
- [ ] **Layers:** per-instance evaluation of bindings, helpers, groups and placements; `forEach` layers and `navigate` with `item`.
- [ ] **Flows:** flow templates and instances, with `forEach`.

### Phase 4: Components and placements (Rust)
- [ ] `component` and `placement` entities; slot fit; owned controls; per-placement bindings, helpers and owned flows with `{slot}` triggers; `parts` with slot mapping; state at `layer.<slug>.parts.<placement>`.
- [ ] Control groups that place a component per item (`use`), with `item` bound into params.

### Phase 5: Transforms from plugins and projects (Rust + SDK)
- [ ] Manifest `transforms` and `components` (plugin protocol §4) in TypeSpec and the SDK; loading transforms into the core's sandbox with type checks; namespacing.
- [ ] The `transform` project entity.
- [ ] Manifest `presets` retired.

### Phase 6: UI
- [ ] One instance panel for every kind: the template it uses, its params form, `forEach`, its live items, "Detach", and where the template is used.
- [ ] A composite editor: params, a state tree of sources, actions (forward or owned flow), events, and where the template is used as a constraint.
- [ ] Owned flows in the layer, composite and component editors.
- [ ] A component editor with parts, placement from a palette onto a layer's keys, and the key editor's component view with the chain of containers.
- [ ] A transform editor with a test bench: feed input sequences, see states, values and events.

### Phase 7: Acceptance
- [ ] A Rust end-to-end test covers the "Done when" list against simulated devices.

## Ordering

Phase 0 first, then Phases 1 and 2. Phases 3 and 5 can run side by side once Phase 2 is in; Phase 4 needs the layer kind from Phase 3 (placements on template layers). Phase 6 follows the APIs, and Phase 7 joins them.

## Out of scope

Composites containing composite instances · surface, control group and variable templates · templates derived from templates · a registry for sharing components and transforms between projects.
