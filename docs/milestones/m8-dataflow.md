# Milestone 8: Dataflow

- **Status:** Planned, after [M7](m7-surfaces-from-plugins.md)

Covalence gets its continuous half. Values can be computed, accumulated and sent somewhere without a flow ([ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md)):
- **Reducers** (`toggle`, `position`, `select` …) are sandboxed transform definitions.
- **Cells** give computed and accumulated values a home in the state tree.
- **Variables** become declared entities that can be computed.
- **Sinks** keep a variable or an action parameter in step with a value.

Helpers on controls are reducers whose input defaults to their control. Layers become active from state, so shift layers and popovers arrive. The virtual surface gains an encoder and a fader, so all of this can be shown without hardware beyond M7's Stream Deck.

## Done when

- [ ] A latching key: a `toggle` helper on `k3`, its background a `rules` source on the helper's value, and a flow that runs on the helper's `on` event.
- [ ] A GPIO-style contact from a test plugin latches the same way, through a variable whose `value` is a `toggle` on the plugin's events. No surface is involved.
- [ ] `vars.isLive` declared as `{expr: "atem.program == 3"}` (against a simulated switcher) is a read-only cell. A binding and a `state` trigger read it, and `why` on the trigger's action passes through the variable's patch to the switcher's.
- [ ] A virtual encoder with a `position` helper that `follow`s `vh`'s simulated volume and has `to: {action: setVolume}`. Turning it sends rate-limited actions with the latest value. When the volume changes from elsewhere, the helper follows it and sends nothing back.
- [ ] A virtual encoder with a `select` helper over `core` surfaces' layers and `to: {action: navigate}` pages a deck.
- [ ] A shift layer (`level: over`, `when: {while: …k15.pressed}`) takes over three keys while `k15` is held and redraws within 16 ms. A latching shift reads a `toggle`.
- [ ] Every built-in reducer has a `defineTransform` definition and a conformance suite. Any native implementation passes it.
- [ ] A cycle between two computed variables is reported as `cycle` on both, and the rest of the turn goes on.
- [ ] A declared variable with `persist: true` keeps its value across a server restart.
- [ ] Budgets hold with cells between a change and a key: state change → redrawn key in under 16 ms with a chain of three cells.

## Phases

### Phase 0: Decisions and docs
- [x] [ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md); [dataflow spec](../specs/dataflow.md) §4.2–§7; surfaces §4.3 and §5, bindings §5 marked **(M8)**.
- [ ] **The graph's home.** Decide whether the binding worker becomes the dataflow evaluator (one thread, one QuickJS) or a separate thread feeds it. The constraint is the 16 ms budget with sandboxed reducers in the path.
- [ ] **Reducer state on failover:** confirm that replicated cell patches are enough to resume, with no separate checkpoint.
- [ ] **Entity plumbing.** A generic `EntityKind` in `covalence-server` (validate, dependencies, cascade on delete, routes) before `variable` joins and M9 adds three more types, so each type isn't another `save_x`/`put_x` pair.

### Phase 1: Transform definitions and reducers (Rust + JS)
- [ ] `defineTransform` and its sandbox context: pure, time passed in, `wakeAt`, the bindings' limits ([dataflow §4.4](../specs/dataflow.md)).
- [ ] Definitions for `toggle`, `counter`, `position`, `velocity`, `select`, `absolute`, `rate`, `latch` and `script`, in `crates/covalence-core/transforms/`. The stateless transforms from M6 get definitions too, as their specification.
- [ ] A conformance runner: the same input sequences through the definition and any native implementation, comparing states, values and events.
- [ ] Native implementations where the benchmark asks for them.

### Phase 2: Cells and the graph (Rust)
- [ ] `covalence_core::dataflow`: cells, dependency-ordered evaluation within a turn, cycle detection, `cause` on cell patches, `{core: "dataflow"}` origin, quality propagation, sampled journaling.
- [ ] Bindings, `state` triggers, `waitUntil` and (Phase 5) layer resolution become consumers of the settled turn.
- [ ] Reducer state: in memory, restored from a persisted variable, resumed from replicated patches.

### Phase 3: Variables and sinks (Rust)
- [ ] The `variable` entity: schema, `initial`, `persist`, `value`, `to`, `journal`. `set` on a computed variable is `readOnly`; an undeclared `set` warns.
- [ ] Sinks: `set`, and `action` with rate limit, latest value wins, `undefined` sends nothing, and no firing on a change that descends from the sink's own action.
- [ ] Persisted variables restored from `state_cache` before triggers are armed.

### Phase 4: Helpers on controls (Rust)
- [ ] `ControlBinding.helpers` and `gestures`. Helper cells under the layer or the surface; helper events at their cell; `{layer, controls}` triggers matching helper event names.
- [ ] `follow`, `holdoff` and `pickup`.
- [ ] The virtual surface gains an optional encoder (`relative` + `momentary`) and fader (`absolute` + `touch`) for tests and the UI.

### Phase 5: Layer resolution (Rust)
- [ ] `when`, `level`, `rank` and `on` as sources and selectors; the resolution rule of surfaces §5.2; `surface.<slug>.layers` and `.controls.<id>.layer`; `resolutionCycle`.
- [ ] Armed layers, evaluated ahead while inactive.
- [ ] Presets: navigation group, levels, shift, popover, always on.

### Phase 6: UI
- [ ] A variables view: declare, type, persist, computed with the source editor, and sinks.
- [ ] Helpers on the key editor: pick a reducer, its options as sources, its sinks, and a live readout of its cell.
- [ ] Layer fields: level, rank, `when` presets, and the control's candidates ("k3: Shift while k15 is held, otherwise Cameras").
- [ ] The virtual deck's encoder and fader.
- [ ] `why` and the timeline show cell patches as links in the chain.

### Phase 7: Acceptance
- [ ] A Rust end-to-end test covers the "Done when" list against simulated devices.
- [ ] Budgets measured with cells and sandboxed reducers in the path.

## Ordering

Phase 0 first. Phases 1 and 2 can run side by side; 3 and 4 need 2; 5 needs 2 and is independent of 3 and 4. Phase 6 follows the APIs, and Phase 7 joins them.

## Out of scope

Plugin- and project-defined transforms, composites, templates and components ([M9](m9-composition.md)) · `led`, `level` and `text` outputs on hardware · control groups and surface groups · `back` history · echo detection by timing (surfaces §4.4).
