# Milestone 8: Dataflow

- **Status:** In progress (Phases 0–3 done)

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
- [x] `defineTransform` and its sandbox context: pure, time passed in, `wakeAt`, the bindings' limits ([dataflow §4.4](../specs/dataflow.md)). `transform::definition`: a fresh context per call, under the expression prelude (no clock, no randomness), with a 10 ms limit. Steps see `{event, input, previous, value, followed, options, now}`; definitions may add `initValue` and `follow`.
- [x] Definitions for `toggle`, `counter`, `position`, `velocity`, `select`, `absolute`, `rate`, `latch` and `script`, in `crates/covalence-core/transforms/`. The stateless transforms from M6 get definitions too, as their specification. A reducer parses as a source (`{use: "toggle@1", on, input, follow, holdoff, …options}`), its options checked against its definition; outside a cell it's `unsupported`.
- [x] A conformance runner (`transform::conformance`): the same input sequences through the definition and any native implementation, comparing states, values and events, with options changed between steps. One suite per built-in, in `transforms/conformance/`. It found that `serde_json` needs `float_roundtrip` to read the sandbox's numbers back exactly.
- [x] Native implementations where the benchmark asks for them: every reducer but `script` (a sandboxed step costs about 2 ms in a debug build, a native one a few µs; `tests/transforms.rs` prints both).

### Phase 2: Cells and the graph (Rust)
- [x] `covalence_core::dataflow`: cells, dependency-ordered evaluation within a turn, cycle detection, `cause` on cell patches, `{core: "dataflow"}` origin, quality propagation, sampled journaling. A cell is a `CellDef` (`id` by entity, `home`, `source`, `journal`), added with `Dataflow::put` and removed with `Dataflow::remove`.
- [x] Bindings, `state` triggers, `waitUntil` and (Phase 5) layer resolution become consumers of the settled turn. The graph is the state tree's **settler**: `LiveState` holds a turn lock across a writer's mutation and the graph's evaluation, and publishes one dirty set with both, so everything on the feed sees settled state without changing how it subscribes. Events, wakes and edits start turns of their own (`LiveState::settle`), events and wakes from the `dataflow` thread. This is the first step to ADR 0033's single turn thread: writers still apply their own patches, under the turn.
- [x] Reducer state: in memory, restored from a persisted variable, resumed from replicated patches. All three go through the leaf: a reducer cell's patch op and leaf carry `reducer: {transform, state?, wakeAt?}`, and a cell with no state in memory resumes from its leaf when `transform` names the same definition and the value still fits its `result`. A standby (`Dataflow::set_active(false)`) never steps; on promotion it resumes from the leaves and evaluates stateless cells once.

Notes from Phase 2:
- The turn's evaluations have a 2 ms limit in release builds, as ADR 0033 says, and 50 ms in debug builds, where building a fresh QuickJS context alone can take longer.
- A reducer whose value is `undefined` (an `absolute` before any input) keeps its metadata on the leaf but can't journal it, since a delete op has no `reducer`. It restarts from `init` after a failover.
- Not yet: the derived journal tier and `inputCause` (state spec §2.4).

### Phase 3: Variables and sinks (Rust)
- [x] The `variable` entity: schema, `initial`, `persist`, `value`, `to`, `journal`. `set` on a computed variable is `readOnly`; an undeclared `set` warns. `covalence_core::variable`, served at `/variables` through the generic entity routes. A `set` step on a computed variable is `readOnly` when the flow is saved and fails with `readOnly` if it runs anyway; one on an undeclared variable warns `undeclaredVariable`. Saving a variable whose static reads (`{ref}`s to `vars` and `vars.<name>` in expressions) already form a cycle is `cycle`.
- [x] Sinks: `set`, and `action` with rate limit, latest value wins, `undefined` sends nothing, and no firing on a change that descends from the sink's own action. A `set` sink writes in the same turn, so cells reading its variable settle with it. An `action` sink's request goes through the core (output gate, journal) with origin `{core: "dataflow"}` and `cause` → the cell's patch. "Its own action" is an `action.requested` from the dataflow to the same connection and action whose `cause` is a patch of the same cell, found within 32 links of `cause`. A cell's first evaluation after it's added, edited, resumed or re-aliased sends nothing.
- [x] Persisted variables restored from `state_cache` before triggers are armed. The store gains a `state_cache` table, keyed by variable ID so a rename keeps it, holding `{value, reducer?}`; the server writes it whenever a persisted variable's value changes, and at startup puts it back, journaled, before cells are added (so reducers resume) and before flows are armed. A writable variable with nothing cached gets its `initial`.

### Phase 4: Helpers on controls (Rust)
- [ ] `ControlBinding.helpers` and `gestures`. Helper cells under the layer or the surface; helper events at their cell; `{layer, controls}` triggers matching helper event names.
  - [x] `helpers`: a map of reducers on the binding, each with `as` (default its key), `scope`, `to` and `journal`. A layer-scoped helper is a cell at `layer.<slug>.controls.<id>.<name>`, identified by `helper:<binding>/<key>`, kept in step with the binding and its layer's slug by the server. Its `on` defaults to `{from: {layer, controls: <id>}}` with the definition's event names; with no `input`, an input-stepped reducer takes the control's `turn` (detents, counted from 0) and `change` (a value) as its input.
  - [x] Helper events are journaled at the cell with the binding's `layer` and `control`, so event triggers on `{layer, controls}` match them by name (the flow runtime now indexes `{layer}` sources under `layer` as well as `surface`). A cell never steps on its own events.
  - [ ] `scope: "surface"` (one cell per surface) is `unsupported` for now.
  - [ ] `gestures`: per-control gesture timing.
- [x] `follow`, `holdoff` and `pickup`. `follow` applies when what it follows changes, through the definition's `follow` (`absolute` drops its pickup) or by taking the value; it pauses while the control is touched and for `holdoff` (default 250 ms; a number of ms or a duration) after the last input, then catches up. A helper starts where what it follows is. Changes from `follow` send nothing to sinks.
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
