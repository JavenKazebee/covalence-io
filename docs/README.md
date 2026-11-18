# Covalence IO documentation

Two kinds of document:

- **Specs** say how Covalence works now, and why. Each ends with a **Decisions** section: what was decided, the reasons, and the alternatives that were rejected.
- **Milestones** are plans: what's being built next, and the checklist that says when it's done. A finished milestone keeps its goal and its "Done when" list, which the acceptance tests cite.

## Specs
- [Overview](specs/overview.md): components, terms, repo layout, performance budgets, and the foundations (Rust, the UI, Orbital, simulation, licence)
- [State model and event log](specs/state-and-event-log.md): the state tree, the journal, the output gate, storage, the execution model, redundancy, multi-user editing, live edits
- [Plugin protocol](specs/plugin-protocol.md): messages, manifests, transports and devices, the Deno host, permissions, dynamic targets, distribution
- [Flows](specs/flows.md): flows, steps, runs, expressions and scripts
- [Dataflow](specs/dataflow.md): sources, selectors, events, transforms, cells, variables, sinks, scope
- [Bindings](specs/bindings.md): how a control looks, evaluation and rendering
- [Surfaces and layers](specs/surfaces.md): surfaces, controls, input, helpers, layers, the web surface
- [The core connection](specs/core-actions.md): what Covalence itself can do, as actions
- [Composition](specs/composition.md): templates and instances, nesting, `forEach`, composites, components

Markers such as **(M8)** in a spec name the milestone that builds a rule; each spec's header says what its markers mean.

## Milestones
- [M1: Walking skeleton](milestones/m1-walking-skeleton.md) (done)
- [M2: First UI slice](milestones/m2-first-ui.md) (done)
- [M3: First flows](milestones/m3-first-flows.md) (done)
- [M4: Expressions](milestones/m4-expressions.md) (done)
- [M5: Surfaces and bindings](milestones/m5-surfaces-and-bindings.md) (done)
- [M6: Primitives](milestones/m6-primitives.md) (done, except re-measuring the redraw budget on the Apple M3): sources, selectors, events by path, the core connection
- [M7: Surfaces from plugins](milestones/m7-surfaces-from-plugins.md) (done, except checks on hardware: Phase 7 waits on a real MK.2, and the budgets on the Apple M3): devices, transports, surfaces over the plugin host, the Stream Deck plugin
- [M8: Dataflow](milestones/m8-dataflow.md) (in progress: Phases 0–4 done): reducers, cells, variables, sinks, helpers, layer resolution
- [M9: Composition](milestones/m9-composition.md) (planned): one template primitive, composites, components, nesting, `forEach`, plugin transforms
