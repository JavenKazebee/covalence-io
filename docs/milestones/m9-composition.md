# Milestone 9: Composition

- **Status:** Planned, after [M8](m8-dataflow.md)

Users build bigger things out of smaller ones, and reuse them by parameters, never by copying ([ADR 0030](../adr/0030-composite-connections.md), [ADR 0031](../adr/0031-templates-instances-and-components.md)):
- **Composite connections** make "Camera A" one target out of a PTZ head, a switcher's tally and a recorder.
- **Templates and instances** replace copies of layers and composites.
- **Components** package a control's look, helpers and flows with parameters. Plugins ship them in place of presets.
- **Transforms from plugins and projects** run in the core's sandbox beside the built-ins ([ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md)).

## Done when

- [ ] A composite "Camera A" over a simulated PTZ camera, a simulated switcher and a simulated recorder: its `tally.program` follows the switcher, `recallPreset` forwards to the PTZ, and `takeLive` runs a flow. `why` on the PTZ's `action.result` walks back through the flow's run to the composite's `action.requested`.
- [ ] The composite implements `camera.ptz@1`, and an event trigger on `{target: {interface: "camera.ptz@1"}}` fires for it as for a real camera.
- [ ] A composite template "Camera" with three instances, one per camera. Editing the template changes all three, and a change that breaks an instance's params is refused, naming it.
- [ ] A layer template "Camera control" with a `camera` param, instanced per camera. One flow on the template handles a key on every instance with that instance's `params.camera`.
- [ ] A "Tally key" component, placed four times on a layer with a different camera each, draws each key's tally, and its owned flow selects that key's camera. The key editor shows "Tally key, camera = Cam B".
- [ ] A control group places the "Tally key" component per camera in a live list.
- [ ] A test plugin ships a component and a transform. The component can be placed; the transform runs in the core (it still works with the plugin host stopped) and is named `<plugin id>/<id>`.
- [ ] A project transform (`project/<slug>`) is defined in the UI and used as a helper.
- [ ] "Detach" turns an instance or a placement into plain entities with no link back.

## Phases

### Phase 0: Decisions and docs
- [x] [ADR 0030](../adr/0030-composite-connections.md), [ADR 0031](../adr/0031-templates-instances-and-components.md); [composition spec](../specs/composition.md); [dataflow §8](../specs/dataflow.md) scope.
- [ ] Interfaces: ship the first ones from real plugins (`camera.ptz@1`, `switcher@1`, `recorder@1`), since composites and components are only as useful as the interfaces they implement. ADR 0019's dynamic targets must be built by now; if not, they come first in this milestone.
- [ ] Owned-flow identity: the journal and API shape of `<component>/<flow>` runs and their placement.

### Phase 1: Flow inputs and outputs (Rust)
- [ ] `inputs` and `outputs` on flows, `stop` with outputs, and the `call` step with `wait`. Composite actions need them.

### Phase 2: Composite connections (Rust)
- [ ] Plugin `core.composite`: `params`, state cells, `connected`, `forward` and `flow` actions, re-emitted events, `implements` with conformance on save.
- [ ] Causation through composite actions and events; nesting, `cycle`, the loop guard and `tooDeep`.

### Phase 3: Templates and instances (Rust)
- [ ] `params` (`ParamDef` with source defaults) and `template` on layers and composites; instance validation (`badInstance`); per-instance evaluation of bindings and helpers; events carrying the instance.
- [ ] `params` in scope (dataflow §8) and `{scope}` refs. The `layer.params` shorthand is rejected.
- [ ] "Detach".

### Phase 4: Components and placements (Rust)
- [ ] `component` and `placement` entities; slot fit; owned controls; per-placement bindings, helpers and owned flows with `{slot}` triggers; state at `layer.<slug>.parts.<placement>`.
- [ ] Control groups that place a component per item, with `item` bound into params.

### Phase 5: Transforms from plugins and projects (Rust + SDK)
- [ ] Manifest `transforms` and `components` (plugin protocol §4) in TypeSpec and the SDK; loading transforms into the core's sandbox with type checks; namespacing.
- [ ] The `transform` project entity.
- [ ] Manifest `presets` retired.

### Phase 6: UI
- [ ] A composite editor: params, a state tree of sources, actions (forward or flow), events, interfaces with a conformance report.
- [ ] Template and instance flows: "Create from template", the params form, "Detach", and where each instance is used.
- [ ] A component editor, placement from a palette onto a layer's keys, and the key editor's component view.
- [ ] A transform editor with a test bench: feed input sequences, see states, values and events.

### Phase 7: Acceptance
- [ ] A Rust end-to-end test covers the "Done when" list against simulated devices.

## Ordering

Phase 0 first, then Phase 1. Phases 2, 3 and 5 can run side by side; Phase 4 needs 3 (placements on template layers). Phase 6 follows the APIs, and Phase 7 joins them.

## Out of scope

Composites over collections (`items`) · nested components · instances of instances · surface and control group templates · a registry for sharing components and transforms between projects.
