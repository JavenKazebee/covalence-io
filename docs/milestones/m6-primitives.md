# Milestone 6: Primitives

- **Status:** Done, except re-measuring the redraw budget on the reference machine (Phase 6)

The shapes M5 stored get their general form before hardware surfaces start creating entities in real projects ([M7](m7-surfaces-from-plugins.md) creates them on plug-in). Three changes:
- **One way to give a value and one way to pick things:** `Source<T>` and `Selector` ([dataflow §10.1](../specs/dataflow.md#101-one-way-to-give-a-value-one-way-to-pick-things)). The binding kinds become stateless transforms.
- **Events are addressed by path,** so every event (a key's, a plugin's, a flow's, the core's) reaches flows through one `event` trigger ([dataflow §10.2](../specs/dataflow.md#102-events-are-addressed-by-state-path)).
- **The core is a connection,** so flows navigate surfaces, inject input and cancel flows with ordinary action steps ([core actions §6.1](../specs/core-actions.md#61-the-core-is-a-connection)).

Nothing here adds a new kind of thing for users to learn. It makes the existing ones combine. Reducers, variables and sinks are [M8](m8-dataflow.md); composites, templates and components are [M9](m9-composition.md).

## Done when

- [x] Every M5 binding fixture is migrated to a `Source` and evaluates to exactly what it did before, natively and as its compiled expression. A `rules` source whose `when` combines two conditions with `all` works on a key.
- [x] A flow with `{kind: "event", from: {layer, controls: "k0"}, event: "press"}` routes `vh` from the virtual deck, and `why` on its action walks back through `trigger.matched` and the key's `event` entry to the input's `state.patch`. M5's `control` trigger fixture is migrated to it.
- [x] One flow with `from: {path: {match: "surface.*.controls.*"}}, event: "press"` fires for any key on any surface, with `trigger.captures` naming the surface and control. A pattern `state` trigger on `surface.*.controls.*.pressed` does the same from state.
- [x] An event trigger on `{target: {interface: …}}` is accepted and matches events from every connection implementing the interface (with a test interface, since none ship yet). Interfaces were later dropped (plugin protocol §13.9); the same test now uses `{target: {plugin: …}}`.
- [x] An event trigger on `flow.<slug>`, `event: "finished"`, runs when that flow finishes, and `{path: "sys"}, event: "startup"` runs at boot.
- [x] A flow presses a key on another layer and navigates the deck: `core` actions `inject` and `navigate`, on a fixed surface target and on a target held in `vars`. Both are journaled `action.requested` → `action.result` and both happen in simulate mode. `cancelRuns` and `setEnabled` work on a flow target.
- [x] Action parameters accept `{ref}` and stateless transforms as well as `{expr}`, and `wait.for` accepts a `Source<Duration>`.
- [x] The UI edits all of the above: one source widget for binding properties and action parameters, an event-source picker in the trigger editor (key, any key on a layer, a connection, a flow, startup, a pattern), and `core` in the action step's connection list.
- [ ] Budgets from M5 still hold: key down → first action dispatched in under 5 ms, state change → redrawn key in under 16 ms. Key down holds; the redraw is unchanged from M5 on the same machine, but needs re-measuring on the M3 (Phase 6).

## Budgets measured

In release, on a 4-vCPU cloud VM (Intel Xeon, 2.1 GHz), and against the code before M6 (`1568d7b`) on the same machine, since M5's figures are from an Apple M3:

| Path | Budget | Before M6 | M6 |
|---|---|---|---|
| Key down → first action requested, layer redrawing | 5 ms | 260 µs | 290 µs on a layer's key or a `*` pattern; 520–610 µs among 200 other event triggers |
| State change → 15 keys at 144 px redrawn, 2 `expr` bindings each | 16 ms | 28–37 ms | 29–38 ms |
| The same at 72 px | 16 ms | 27–30 ms | 28–30 ms |

`bench_key_down_to_first_action` now runs on event triggers, with and without 200 others: triggers under the same root (`surface`) are each tested, at about 2.5 µs, and reparsing a pattern's text per test is the first thing to cache if that grows. The redraw path is unchanged by M6 and machine-bound here: this VM is about 3× slower than the M3, where M5 measured 8.7 ms. **Open:** re-run `cargo test --release -p covalence-core --test binding_worker bench -- --ignored --nocapture --test-threads=1` on the M3 to confirm the 16 ms budget.

## History

The phase-by-phase build log was trimmed once the milestone closed. It's in git: `git log -p -- docs/milestones/m6-primitives.md`.
