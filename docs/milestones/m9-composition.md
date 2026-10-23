# Milestone 9: Composition

- **Status:** Planned, after [M8](m8-dataflow.md)

Users build bigger things out of smaller ones, and reuse them by parameters, never by copying ([ADR 0030](../adr/0030-composite-connections.md), [ADR 0031](../adr/0031-templates-instances-and-components.md), [ADR 0032](../adr/0032-one-template-primitive.md)):
- **One template primitive.** Layers, composite connections, components and flows declare `params`, and instances bind them. Binding, scope, identity, validation and Detach are built once, for all four.
- **Composite connections** make "Camera A" one target out of a PTZ head, a switcher's tally and a recorder.
- **Templates nest,** so a camera strip is made of tally keys, and **`forEach`** makes one instance per item of a live list: a composite per camera, a watchdog flow per recorder.
- **Components** package a control's look, helpers and flows with parameters. Plugins ship them in place of presets.
- **Transforms from plugins and projects** run in the core's sandbox beside the built-ins ([ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md)).

## Done when

- [ ] A composite "Camera A" over a simulated PTZ camera, a simulated switcher and a simulated recorder: its `tally.program` follows the switcher, `recallPreset` forwards to the PTZ, and `takeLive` runs a flow it owns. `why` on the PTZ's `action.result` walks back through the flow's run to the composite's `action.requested`.
- [ ] The composite implements `camera.ptz@1`, and an event trigger on `{target: {interface: "camera.ptz@1"}}` fires for it as for a real camera.
- [ ] A composite template "Camera" with three instances, one per camera. Editing the template changes all three, and a change that breaks an instance's params is refused, naming it.
- [ ] One "Cameras" composite instance with `forEach` over a list in `vars`. Each item is a `camera.ptz@1` target at `cams.items[<key>]`. Adding an item to the list adds a camera, journaled `instance.added`; removing one ends it, and its owned flow's active run is cancelled with `removed`.
- [ ] A layer template "Camera control" with a `camera` param, instanced per camera. One flow owned by the template handles a key on every instance with that instance's `params.camera`. A free-standing flow on the same key gets `trigger.instance` but no `params`.
- [ ] A "Tally key" component, placed four times on a layer with a different camera each, draws each key's tally, and its owned flow selects that key's camera. The key editor shows "Tally key, camera = Cam B".
- [ ] A "Camera strip" component with a "Tally key" part, placed on a layer template's instance. The part's tally follows the strip's camera, which follows the layer instance's. `why` on its owned flow's action shows the chain: layer instance, placement, part. A component that contains itself can't be saved (`recursiveTemplate`).
- [ ] A control group places the "Tally key" component per camera in a live list.
- [ ] A flow template "Recorder watchdog" with a `recorder` param, instanced with `forEach: {targets: {interface: "recorder@1"}}`, runs once per recorder, including one connected during the test. A `call` to a flow with `inputs` still binds them per run.
- [ ] A test plugin ships a component and a transform. The component can be placed; the transform runs in the core (it still works with the plugin host stopped) and is named `<plugin id>/<id>`.
- [ ] A project transform (`project/<slug>`) is defined in the UI and used as a helper.
- [ ] "Detach" turns an instance or a placement of each kind into plain entities with no link back, and a `forEach` instance into one entity per current item.

## Phases

### Phase 0: Decisions and docs
- [x] [ADR 0030](../adr/0030-composite-connections.md), [ADR 0031](../adr/0031-templates-instances-and-components.md); [composition spec](../specs/composition.md); [dataflow §8](../specs/dataflow.md) scope.
- [x] [ADR 0032](../adr/0032-one-template-primitive.md): one template primitive, owned flows, nesting, `forEach`, the `instance` chain. Composition spec v0.2.
- [ ] Interfaces: ship the first ones from real plugins (`camera.ptz@1`, `switcher@1`, `recorder@1`), since composites and components are only as useful as the interfaces they implement. ADR 0019's dynamic targets must be built by now; if not, they come first in this milestone.
- [ ] The `instance` chain's API shape: how the timeline, `why` and the runs list show and filter it.

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
- [ ] **Composites:** plugin `core.composite`: `params`, state cells, `connected`, `forward` and owned-flow actions, re-emitted events, `implements` with conformance on save. Causation through composite actions and events; `cycle`, the loop guard and `tooDeep`. Collections through `forEach`, with interfaces per item through `at`.
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
- [ ] A composite editor: params, a state tree of sources, actions (forward or owned flow), events, interfaces with a conformance report.
- [ ] Owned flows in the layer, composite and component editors.
- [ ] A component editor with parts, placement from a palette onto a layer's keys, and the key editor's component view with the chain of containers.
- [ ] A transform editor with a test bench: feed input sequences, see states, values and events.

### Phase 7: Acceptance
- [ ] A Rust end-to-end test covers the "Done when" list against simulated devices.

## Ordering

Phase 0 first, then Phases 1 and 2. Phases 3 and 5 can run side by side once Phase 2 is in; Phase 4 needs the layer kind from Phase 3 (placements on template layers). Phase 6 follows the APIs, and Phase 7 joins them.

## Out of scope

Composites containing composite instances · surface, control group and variable templates · templates derived from templates · a registry for sharing components and transforms between projects.
