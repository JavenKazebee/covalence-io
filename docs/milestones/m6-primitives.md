# Milestone 6: Primitives

- **Status:** In progress (Phases 0 and 1 done)

The shapes M5 stored get their general form before hardware surfaces start creating entities in real projects ([M7](m7-surfaces-from-plugins.md) creates them on plug-in). Three changes:
- **One way to give a value and one way to pick things:** `Source<T>` and `Selector` ([ADR 0026](../adr/0026-sources-and-selectors.md)). The binding kinds become stateless transforms.
- **Events are addressed by path,** so every event (a key's, a plugin's, a flow's, the core's) reaches flows through one `event` trigger ([ADR 0027](../adr/0027-events-addressed-by-path.md)).
- **The core is a connection,** so flows navigate surfaces, inject input and cancel flows with ordinary action steps ([ADR 0028](../adr/0028-the-core-is-a-connection.md)).

Nothing here adds a new kind of thing for users to learn. It makes the existing ones combine. Reducers, variables and sinks are [M8](m8-dataflow.md); composites, templates and components are [M9](m9-composition.md).

## Done when

- [ ] Every M5 binding fixture is migrated to a `Source` and evaluates to exactly what it did before, natively and as its compiled expression. A `rules` source whose `when` combines two conditions with `all` works on a key.
- [ ] A flow with `{kind: "event", from: {layer, controls: "k0"}, event: "press"}` routes `vh` from the virtual deck, and `why` on its action walks back through `trigger.matched` and the key's `event` entry to the input's `state.patch`. M5's `control` trigger fixture is migrated to it.
- [ ] One flow with `from: {path: {match: "surface.*.controls.*"}}, event: "press"` fires for any key on any surface, with `trigger.captures` naming the surface and control. A pattern `state` trigger on `surface.*.controls.*.pressed` does the same from state.
- [ ] An event trigger on `{target: {interface: …}}` is accepted and matches events from every connection implementing the interface (with a test interface, since none ship yet).
- [ ] An event trigger on `flow.<slug>`, `event: "finished"`, runs when that flow finishes, and `{path: "sys"}, event: "startup"` runs at boot.
- [ ] A flow presses a key on another layer and navigates the deck: `core` actions `inject` and `navigate`, on a fixed surface target and on a target held in `vars`. Both are journaled `action.requested` → `action.result` and both happen in simulate mode. `cancelRuns` and `setEnabled` work on a flow target.
- [ ] Action parameters accept `{ref}` and stateless transforms as well as `{expr}`, and `wait.for` accepts a `Source<Duration>`.
- [ ] The UI edits all of the above: one source widget for binding properties and action parameters, an event-source picker in the trigger editor (key, any key on a layer, a connection, a flow, startup, a pattern), and `core` in the action step's connection list.
- [ ] Budgets from M5 still hold: key down → first action dispatched in under 5 ms, state change → redrawn key in under 16 ms.

## Phases

### Phase 0: Decisions and docs
- [x] [ADR 0026](../adr/0026-sources-and-selectors.md), [ADR 0027](../adr/0027-events-addressed-by-path.md) and [ADR 0028](../adr/0028-the-core-is-a-connection.md). New specs: [dataflow](../specs/dataflow.md) (§1–§4.1 for M6) and [core actions](../specs/core-actions.md). Surfaces, bindings, flows, state and plugin protocol specs marked **(M6)**.
- [x] **Migration policy.** There are no real projects yet, so nothing is migrated: the stored shapes change, dev databases are reset, and fixtures are rewritten. Migrations ([ADR 0006](../adr/0006-project-storage-and-versioning.md)) start with the first real deployment.
- [x] **[ADR 0033](../adr/0033-execution-model.md), the execution model.** Accepted before the evaluator is reshaped: the turn thread, its QuickJS runtime, the script pool and the drawing thread.
- [x] **Source recognition** ([dataflow §1.1](../specs/dataflow.md)): a hand-written `Deserialize` that dispatches on the reserved key (`ref`, `expr`, `use`, `value`) and deserializes `T` only for literals, rather than `#[serde(untagged)]`. A reserved key commits the object: keys its form doesn't allow are `invalid`, never a literal. `use` decides by itself, since a transform's options may share a reserved name (`compare`'s `value`).
- [x] ADR 0024, which M5 built, accepted, so the ADRs this milestone amends are settled first. ADRs 0026, 0027 and 0028 accepted as Phase 0 closes.

### Phase 1: Sources and stateless transforms (Rust)
- [x] `covalence_core::source`: `Source<T>` (serde, recognition, `fallback`), `Ref` with `conn` and `root`, evaluation with read tracking (one evaluator; one-shot and reactive positions differ only in when they call it), `to_expr`, and a walk over nested parts for validation. A `ref` through a `target` stays `unsupported` in bindings, as in M5; in flow parameters it doesn't parse yet (`invalid`).
- [x] `covalence_core::transform`: the stateless transforms `format`, `template`, `rules`, `compare`, `all`, `any`, `not` and `map`, moved from `binding` and keyed by `use`. `tests/bindings.rs`'s native-versus-expression suite runs over every transform, chained ones and nested fallbacks included. `all` and `any` stop at the first item that decides them, as `&&` and `||` do.
- [x] `ControlBinding.props` become `Source<Value>`. The binding worker evaluates sources. `fixtures/bindings/` migrated, with problem fixtures for each `use`; every migrated M5 fixture evaluates to what it did before.
- [x] Flow action `params` and `wait.for` become sources, and `target` a `Source<ConnRef>`. A target that follows state is `unsupported` until ADR 0019's dynamic targets; its `constraint` moves to Phase 4, which is the first to need one (`core` targets held in `vars`).
- [ ] **Left for Phase 5:** the UI's binding model and editor still use M5's shapes, so `bindings/model.test.ts` fails on the migrated fixtures and the binding editor saves shapes the server now rejects.

### Phase 2: Selectors (Rust)
- [ ] `covalence_core::select`: `Selector<Id>` with the shared forms (ID, list, `expr`, `any`, `all`, `not`) and resolvers for controls (`region`, `near`, `has`), targets (`plugin`, `interface`) and paths (`match`, with captures). Stable domain order.
- [ ] Tests per domain, including selectors whose `expr` follows state and re-resolve.

### Phase 3: Events by path (Rust)
- [ ] The `event` journal record gains `path` and context fields; `input.event` is folded into it. Control events are journaled at `surface.<slug>.controls.<id>`.
- [ ] `event.emit` accepts `at` (plugin protocol §6) in TypeSpec, the Rust protocol types and the SDK.
- [ ] Flows emit `started`, `finished` and `failed`; the core emits `sys:startup`.
- [ ] One event matcher in the flow runtime, indexed by path pattern, serving `{path}`, `{target}` (with interface name mapping), `{layer, controls}` and `{surface, controls}` sources. The `control` trigger kind is removed, and `fixtures/flows/control.json` migrated.
- [ ] Pattern `state` triggers (`match`), with `{path, captures, value, previous, ts}`.
- [ ] The trigger payload of dataflow §3.1, and validation: `unknownLayer`, `unknownControl`, `unsupported` across a control selector.

### Phase 4: The core connection (Rust)
- [ ] `core` reserved as an ID, alias and root. The engine routes `conn: "core"` to an in-process handler; the `Actions` trait is unchanged.
- [ ] The `core` manifest in `GET /plugins`, with `surface@1` and `flow@1` as interfaces in `protocol/`.
- [ ] `navigate` (sharing `POST /surfaces/{id}/current`'s code), `inject` (through the surface's input path, origin the run), `cancelRuns` and `setEnabled`.
- [ ] `ActionDef.effect`, and the gate suppressing only `device` effects in simulate mode.
- [ ] Targets held in `vars` (`kind: target`) for `core` surfaces, resolved and checked against the constraint at run time (`no_target`).

### Phase 5: UI
- [ ] One source editor (`SourceInput`): literal, path, expression, and the transform forms, with the compiled expression shown and "Edit as expression". Used for binding properties and action parameters.
- [ ] The trigger editor's event-source picker, replacing "Add key trigger", with patterns shown as text paths.
- [ ] `core` in the action step's target list, with its actions' forms from the manifest. "Navigate to layer" and "Press key" read naturally in the Button view.
- [ ] TS mirrors of the new shapes; `bindings/model.test.ts` and `flows/model.test.ts` round-trip the migrated fixtures.
- [ ] Journal summaries for `event` entries by path.

### Phase 6: Acceptance
- [ ] The Rust end-to-end test from M5 (`surfaces_api.rs`) runs on the migrated shapes, plus a flow that navigates the deck and one that injects a press.
- [ ] Budgets measured again with the selector-based matcher.

## Ordering

Phase 0 comes first. Phases 1, 2 and 4 can run side by side. Phase 3 needs Phase 2's path selectors. Phase 5 follows the APIs it shows, and Phase 6 joins them.

## Out of scope

Reducers, helpers, variables as entities and sinks ([M8](m8-dataflow.md)) · layer resolution beyond navigation ([M8](m8-dataflow.md)) · composites, templates and components ([M9](m9-composition.md)) · dynamic targets through ADR 0019 interfaces on plugin connections, beyond matching their events · control groups, surface groups and selector forms on a layer's `controls` · `back` and `setBrightness`.
