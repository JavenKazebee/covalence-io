# Milestone 8: Dataflow

- **Status:** In progress (Phase 0 done)

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
- [ ] The virtual fader in motor mode: `value` bound to `vh`'s simulated volume moves it, and the fader's own report of that move is an echo (surfaces §4.4). It updates `value` with `cause` → the output's patch, gives no `change`, and its `absolute` helper sends nothing. Touching and moving it sends rate-limited actions again, and `requested.level` and `value` are both readable.
- [ ] A virtual encoder with a `select` helper over `core` surfaces' layers and `to: {action: navigate}` pages a deck.
- [ ] A shift layer (`level: over`, `when: {while: …k15.pressed}`) takes over three keys while `k15` is held and redraws within 16 ms. A latching shift reads a `toggle`.
- [ ] (Proposed) The same shift layer with `cascade: through` binds only `label` and claims input: the keys keep the base layer's background and tally, and `why` on k3's background names the base layer. A lock layer that binds an icon and `input: block` leaves every key's look live and ignores presses. Two layers that tie on a control get `ambiguousOrder` in the editor.
- [ ] Every built-in reducer has a `defineTransform` definition and a conformance suite. Any native implementation passes it.
- [ ] A cycle between two computed variables is reported as `cycle` on both, and the rest of the turn goes on.
- [ ] A declared variable with `persist: true` keeps its value across a server restart.
- [ ] Budgets hold with cells between a change and a key: state change → redrawn key in under 16 ms with a chain of three cells.

## Phases

### Phase 0: Decisions and docs
- [x] [ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md); [dataflow spec](../specs/dataflow.md) §4.2–§7; surfaces §4.3 and §5, bindings §5 marked **(M8)**.
- [x] **The graph's home.** The turn thread ([ADR 0033](../adr/0033-execution-model.md)): the binding worker becomes part of it, with its own QuickJS runtime.
- [x] **Reducer state on failover:** replicated cell patches are enough, with no separate checkpoint, because a reducer's state travels with its value. A cell's patch op carries `reducer: {state, wakeAt?}` when the state differs from the value (`select` over `items`, `velocity`, `absolute` with `pickup`). The standby applies the patches and never steps a reducer. On promotion, reducers resume from that metadata and stateless cells are evaluated once, with no sink firing. A standby further behind than the memory window resyncs from a state snapshot. Persisted variables keep the same metadata in `state_cache`. [Dataflow §5.2](../specs/dataflow.md), [state and event log §1.3, §4](../specs/state-and-event-log.md).
- [x] **Kept state through edits** ([ADR 0034](../adr/0034-live-edits.md), revised): a cell is its entity (a variable's ID, or a binding's ID and the helper's key), and it keeps its value and state when its transform is the same `use` at the same `version` and its value still validates. Changed options apply from the next step, and the conformance suite changes options between steps. Otherwise it resets, with `cause` → a new `project.applied` journal entry, since a journal `cause` can't name a project op. Edits never fire sinks. [Dataflow §5.4](../specs/dataflow.md).
- [x] **Journal tiers** ([state and event log §2.4](../specs/state-and-event-log.md), no longer proposed): inputs reach the disk, and derived entries (cells, helpers, layer resolution, `trigger.matched`) stay in the memory ring. One `seq` for both, so the disk has gaps. An entry whose `cause` is derived also records `inputCause`, so `why` on disk steps over derived entries. Inputs are written in one transaction per turn, or with `journal.flushInterval` (`100ms` on a Pi), at most one per interval.
- [x] **Entity plumbing.** `covalence_server::entity`: an `EntityKind` per type (parse, check, apply, unapply, references) and one `App::save`, `App::put`, `App::delete` and set of five routes for all of them. A reference says what happens when its target goes, `Cascade` (a binding with its layer) or `Clear` (a surface's start layer), and a delete applies that across every type in `KINDS`. Surfaces, layers, control bindings and flows are on it. Connections aren't yet: they're created through the plugin host and have no `PUT` or `DELETE`. `variable` joins in Phase 3.
- [x] [ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md) accepted. ADR 0034 stays Proposed until M9's Phase 0, which settles changesets and pinned runs.

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
- [ ] Echoes (surfaces §4.4): `requested.<output>`, echo marking by the host (`echo: true`) and by `echoWindow`, no events from echoes unless a trigger opts in.
- [ ] The virtual surface gains an optional encoder (`relative` + `momentary`) and fader (`absolute` + `touch`, with a `level` output and a motor mode that echoes) for tests and the UI.

### Phase 5: Layer resolution (Rust)
- [ ] `when`, `level`, `rank` and `on` as sources and selectors; the resolution rule of surfaces §5.2; `surface.<slug>.layers` and `.controls.<id>.layer`; `resolutionCycle`.
- [ ] (Proposed) `cascade: through`: per-property resolution and input as a property (surfaces §5.2.1), `.controls.<id>.look`, the binding's `input`, and `ambiguousOrder`.
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

Plugin- and project-defined transforms, composites, templates and components ([M9](m9-composition.md)) · `led`, `level` and `text` outputs on hardware · control groups and surface groups · `back` history.
