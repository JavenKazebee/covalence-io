# Milestone 9: Composition

- **Status:** In progress: the template primitive is built for flows, components replace layers and own flows, components are templates shown through instances, which run their template's owned flows, and components contain children at any depth; component params, children with `forEach` and composites are next

Users build bigger things out of smaller ones, and reuse them by parameters, never by copying ([composition §11.4](../specs/composition.md#114-composite-connections), [composition §11.1](../specs/composition.md#111-reuse-is-by-parameters-never-by-copying-or-overriding), [composition §11.2](../specs/composition.md#112-one-template-primitive-for-three-kinds)):
- **One template primitive.** Composite connections, components and flows declare `params`, and instances bind them. Binding, scope, identity, validation and Detach are built once, for all three.
- **Composite connections** make "Camera A" one target out of a PTZ head, a switcher's tally and a recorder, and a composite template lets one page control cameras from different plugins.
- **One component at every size** ([composition §11.8](../specs/composition.md#118-one-component-for-keys-strips-and-pages)): a key, a strip and a page are components, with bindings, helpers, owned flows and parameters, and bigger ones contain smaller ones as children. They replace layers, control bindings as entities, placements, parts, control groups and layer templates. Plugins ship them in place of presets.
- **Templates nest,** so a camera strip is made of tally keys, and **`forEach`** makes one instance per item of a live list: a composite per camera, a key per camera, a watchdog flow per recorder.
- **Transforms from plugins and projects** run in the core's sandbox beside the built-ins ([dataflow §10.3](../specs/dataflow.md#103-sources-transforms-and-sinks-helpers-are-transforms)).

## Done when

- [ ] A composite "Camera A" over a simulated PTZ camera, a simulated switcher and a simulated recorder: its `tally.program` follows the switcher, `recallPreset` forwards to the PTZ, and `takeLive` runs a flow it owns. `why` on the PTZ's `action.result` walks back through the flow's run to the composite's `action.requested`.
- [ ] A step constrained to `{template: <Camera>}` accepts "Camera A" and refuses the bare PTZ. An event trigger on `{target: {template: <Camera>}}` fires for every instance.
- [ ] Two simulated PTZ plugins with different preset actions behind one "Camera" template: its `recallPreset` is an owned flow that branches on `target(params.ptz).plugin`, and one layer recalls presets on both. A step constrained to `{plugin: …}` works on a selected camera of that plugin with no composite.
- [ ] A composite template "Camera" with three instances, one per camera. Editing the template changes all three, and a change that breaks an instance's params is refused, naming it.
- [ ] One "Cameras" composite instance with `forEach` over a list in `vars`. Each item is a target at `cams.items[<key>]`, accepted by `{template: <Camera>}`. Adding an item to the list adds a camera, journaled `instance.added`; removing one ends it, and its owned flow's active run is cancelled with `removed`.
- [x] A shown component template "Camera control" with a `camera` param, instanced per camera. One flow owned by the template handles a key on every instance with that instance's `params.camera`. A free-standing flow on the same key gets `trigger.instance` and `trigger.component` but no `params`.
- [ ] A "Camera strip" with a `recordKey` component param, a child of a page three times with one bound to a different record key: that strip's record key is the other component, a fix to the strip reaches all three, and a record key whose controls don't fit is refused with `badInstance`.
- [ ] A "Tally key" component, a child of a page four times with a different camera each, draws each key's tally, and its owned flow selects that key's camera. One event trigger `{component: <Tally key>}` fires for all four. The key editor shows "Tally key, camera = Cam B".
- [ ] A "Camera strip" component with a "Tally key" child, inside an instance of a shown component template. The tally follows the strip's camera, which follows the page instance's. `why` on its owned flow's action shows the chain: page instance, strip, key. A component that contains itself can't be saved (`recursiveTemplate`).
- [ ] A child with `forEach` puts a "Tally key" on the next key per camera in a live list, scrolls when there are more cameras than keys, and a press carries `trigger.item`.
- [ ] A flow template "Recorder watchdog" with a `recorder` param, instanced with `forEach: {targets: {plugin: <recorder sim>}}`, runs once per recorder, including one connected during the test. A `call` to a flow with `inputs` still binds them per run.
- [ ] A test plugin ships a component and a transform. The component can be placed; the transform runs in the core (it still works with the plugin host stopped) and is named `<plugin id>/<id>`.
- [ ] A project transform (`project/<slug>`) is defined in the UI and used as a helper.
- [ ] "Detach" turns an instance of each kind, and a child, into plain entities with no link back, and a `forEach` instance into one entity per current item.

## Phases

### Phase 0: Decisions and docs
- [x] Composites and reuse by parameters drafted in the [composition spec](../specs/composition.md) (§11.1, §11.4), and scope in [dataflow §8](../specs/dataflow.md).
- [x] One template primitive ([composition §11.2](../specs/composition.md#112-one-template-primitive-for-three-kinds)): owned flows, nesting, `forEach`, the `instance` chain. Composition spec v0.2.
- [x] Interfaces dropped for now, the core's included (they only named its collections), and composite templates are how devices from different plugins are controlled as one kind ([plugin protocol §13.9](../specs/plugin-protocol.md#139-dynamic-targets-proposed), [composition §6.5](../specs/composition.md#65-templates-as-contracts)).
- [x] A target is a connection and nothing within it: `core`'s surfaces and flows became `surface` and `flow` parameters ([core actions §2](../specs/core-actions.md#2-surfaces-and-flows-are-parameters)), and plugins don't declare collections ([plugin protocol §13.9](../specs/plugin-protocol.md#139-dynamic-targets-proposed)).
- [x] Dynamic targets for plugins ([plugin protocol §13.9](../specs/plugin-protocol.md#139-dynamic-targets-proposed)): a step's target can follow state with a `{plugin}` constraint, checked against the plugin's manifest on save and against the connection's plugin when the step runs. The step editor offers "the <plugin> in a variable".
- [x] **Edits across entities** ([state spec §7.6](../specs/state-and-event-log.md#76-editing-a-running-show)): edits stay live, and one that spans entities is saved as a batch, validated as a whole and applied all or nothing; runs record the revision they started on.
- [x] The `instance` chain's API shape ([composition §5](../specs/composition.md#5-identity-validation-and-detach)): IDs in the chain, names resolved on read, params recorded on `run.started`, and filters by instance, item and template on the journal and the runs list.
- [x] The open questions the template primitive depends on: a fixed cap of 256 items, each `forEach` composite item a connection with a derived ID, and params that hold a component or a flow ([composition §1.2](../specs/composition.md#12-params-that-hold-a-component-or-a-flow)).
- [x] Accept the composition design ([composition §11](../specs/composition.md#11-decisions)) and live edits ([state spec §7.6](../specs/state-and-event-log.md#76-editing-a-running-show)): drop "proposed" from their headings.
- [x] One component at every size ([composition §11.8](../specs/composition.md#118-one-component-for-keys-strips-and-pages)): components replace layers, control bindings as entities, placements, parts, control groups and layer templates; only shown components take part in resolution; the `layer` root becomes `component`. Composition spec v0.3, surfaces spec v0.6.

### Phase 1: Flow inputs and outputs (Rust)
- [x] `inputs` (as `ParamDef`s) and `outputs` on flows, `stop` with outputs, and the `call` step with `wait`. Composite actions need them. Flows with a `call` trigger can be called, `POST /flows/{id}/run` takes `{inputs}`, and the flow editor edits inputs, outputs, `call` and `stop` ([flows §8.3](../specs/flows.md#83-steps)).

### Phase 2: The template primitive (Rust)
- [x] `covalence_core::compose`: `ParamDef` (source defaults, `required`), `template` and `params` on an instance, binding in the placer's scope, homes (composition §1.1), abstract templates, `badInstance` checked across every instance at every depth. Params are cells at `<home>.params.<name>`. A template with instances can't be deleted.
- [x] `params` in scope (dataflow §8) and `{scope}` refs (`params`, `self`, `item`), checked on save (`notInScope`, `unknownParam`).
- [x] Owned flows on components: `owner`, status under the component's home, `from: {controls}` with the component implied, and deleted with the component.
- [x] Owned flows per instance, for components: a template's owned flow runs once per instance (and for the template itself unless it's abstract), each with its own home, `params` and instance chain; control input on an instance's controls is journaled inside it, and `{component: <template>}` triggers match every instance. Composites' come with them (Phase 3).
- [x] Nesting: instances inside a template's body, `recursiveTemplate`, `tooDeep`. Built for components' children (Phase 4); composites come with them (Phase 3).
- [x] Batches: `POST /batch`, validated as a whole and saved all or nothing.
- [ ] Batches as one `batch` op in one turn, with base revisions, one timeline entry and one undo. Needs the project op log (state spec §2.3), which isn't built.
- [x] `forEach`: per-item instances at `<home>.items[<key>]`, created and ended with the list, `instance.added` and `instance.removed`, cancellation with `removed`, `tooMany`.
- [x] The `instance` chain on journal entries and `trigger.instance`, `params` on `run.started`, and the `instance`, `key` and `template` filters on `GET /journal` and `GET /runs`. Entries from the API name each link.
- [x] Params that hold a flow: `{param}` in a `call` step, literal bindings, and the fit checks. Component params come with components (Phase 4).
- [ ] "Detach", for every kind. Flows have it: `POST /flows/{id}/detach`.

### Phase 3: Template kinds (Rust)
- [ ] `target(t)` in expressions (`.alias`, `.name`, `.plugin`, `.connected`, `.supports(action)` and the connection's state), `kind: "target"` in state schemas, and constraints on sinks' targets ([plugin protocol §13.9](../specs/plugin-protocol.md#139-dynamic-targets-proposed)). Composites read their parts through `target()`.
- [ ] **Composites:** plugin `core.composite`: `params`, state cells, `connected`, `forward` and owned-flow actions, re-emitted events, `{template}` constraints and selectors. Causation through composite actions and events; `cycle`, the loop guard and `tooDeep`. Collections through `forEach`, each item accepted by `{template}`.
- [x] **Flows:** flow templates and instances, with `forEach`. An instance runs its template's steps with its identity; each `forEach` item runs as `<instance>[<key>]`. The UI lists instances but can't edit them yet (Phase 6).

### Phase 4: Components (Rust)
- [x] **Components' names** ([composition §7.4](../specs/composition.md#74-what-components-replace)): the `component` root, `surface.<slug>.shown`, `.controls.<id>.component`, `startComponent`, `{component}` event sources, `trigger.component` and the event's `component`, helpers' `scope: "component"`, `navigate {component}` and `POST /surfaces/{id}/current {component}`, `unknownComponent`. The M5 and M8 tests pass on the new names. Projects saved before aren't migrated (composition §11.8).
- [x] **Layers become components:** the `component` entity with embedded bindings and `show`, and `/components`, `/components/problems` and `/components/preview`. A shown component is resolved, cascaded, armed and navigated as M8's layers were; one without `show` isn't a candidate. Bindings are keyed `<component>/<binding>` in the running core, so their helpers' cells never meet. The UI's layer editor is a component editor: it edits `show`, and saves a binding by putting its component back.
- [x] Component templates and shown instances: a component with `params` is shown by itself with their defaults, or through instances, which are stored as their identity, `template`, `params` and `show`. Each one's params are cells at `component.<slug>.params`, read as `params` by its bindings, helpers, gestures and `show`; `badInstance`, `unboundParam` for a shown abstract template, and `409` on deleting a template with instances.
- [ ] Shown `forEach` instances and `navigate` with `item`.
- [x] Children: control mapping and fit (`doesNotFit`, `unmappedControl`), owned controls (`duplicateControl`), homes under `parts`, helpers' cells under each instance, owned flows with `{controls}` triggers, `{component}` triggers matching every use, the instance chain on control events, nesting, `recursiveTemplate` and `tooDeep`.
- [ ] Params that hold a component: `{param}` as a child's `template`, and the fit check.
- [ ] Children with `forEach`: one item per control of `over`, `item` bound into params, `overflow: "scroll"` with its offset, and `item`, `index` and `key` in `trigger`.

### Phase 5: Transforms from plugins and projects (Rust + SDK)
- [ ] Manifest `transforms` and `components` (plugin protocol §4) in TypeSpec and the SDK; loading transforms into the core's sandbox with type checks; namespacing.
- [ ] The `transform` project entity.
- [ ] Manifest `presets` retired.

### Phase 6: UI
- [ ] One instance panel for every kind: the template it uses, its params form, `forEach`, its live items, "Detach", and where the template is used.
- [ ] A composite editor: params, a state tree of sources, actions (forward or owned flow), events, and where the template is used as a constraint.
- [ ] Owned flows in the composite and component editors.
- [ ] The component editor: a shown component's preset (page, shift, popover, overlay, section) instead of its raw `show` fields, with the fields under "Advanced"; children from a palette onto its keys; "Save as component" from a key; and the key editor's chain of containers.
- [ ] A transform editor with a test bench: feed input sequences, see states, values and events.

### Phase 7: Acceptance
- [ ] A Rust end-to-end test covers the "Done when" list against simulated devices.

## Ordering

Phase 0 first, then Phases 1 and 2. Phases 3, 4 and 5 can run side by side once Phase 2 is in; Phase 4's renames land first within it, since everything after them is built on components rather than layers. Phase 6 follows the APIs, and Phase 7 joins them.

## Out of scope

Composites containing composite instances · a child `when` (composition §10, question 8) · surface and variable templates · templates derived from templates · a registry for sharing components and transforms between projects.
