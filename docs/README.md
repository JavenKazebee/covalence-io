# Covalence IO documentation

## Start here
- [Architecture overview](architecture.md): components, core concepts and repo layout.

## Milestones
- [M1: Walking skeleton](milestones/m1-walking-skeleton.md) (done)
- [M2: First UI slice](milestones/m2-first-ui.md) (done)
- [M3: First flows](milestones/m3-first-flows.md) (done)
- [M4: Expressions](milestones/m4-expressions.md) (done)
- [M5: Surfaces and bindings](milestones/m5-surfaces-and-bindings.md) (done)
- [M6: Primitives](milestones/m6-primitives.md) (planned): sources, selectors, events by path, the core connection
- [M7: Surfaces from plugins](milestones/m7-surfaces-from-plugins.md) (planned)
- [M8: Dataflow](milestones/m8-dataflow.md) (planned): reducers, cells, variables, sinks, helpers, layer resolution
- [M9: Composition](milestones/m9-composition.md) (planned): one template primitive, composites, components, nesting, `forEach`, plugin transforms

## Specs (drafts)
- [State model and event log](specs/state-and-event-log.md)
- [Plugin protocol](specs/plugin-protocol.md)
- [Flows](specs/flows.md)
- [Bindings](specs/bindings.md)
- [Surfaces and layers](specs/surfaces.md)
- [Dataflow](specs/dataflow.md): sources, selectors, events, transforms, cells, sinks, scope
- [The core connection](specs/core-actions.md)
- [Composition](specs/composition.md): templates and instances, nesting, `forEach`, composites, layers, components, flows

## Architecture decision records

| # | Decision | Status |
|---|---|---|
| [0001](adr/0001-rust-core.md) | Core engine in Rust | Accepted (amended by 0025) |
| [0002](adr/0002-protocol-first-plugins.md) | Plugins defined protocol-first | Accepted (amended by 0025) |
| [0003](adr/0003-plugin-runtime-deno.md) | Plugins share one Deno runtime, with per-connection isolation | Accepted (spike deferred by 0015; amended by 0025) |
| [0004](adr/0004-javascript-expressions-and-scripts.md) | JS/TS for expressions and scripts (QuickJS in the core) | Accepted (amended by 0021) |
| [0005](adr/0005-web-ui-vue-tauri-shell.md) | Vue 3 web UI served by the core, with an optional Tauri shell | Accepted |
| [0006](adr/0006-project-storage-and-versioning.md) | SQLite working store, text export, migrations | Accepted |
| [0007](adr/0007-event-sourced-core.md) | Event-sourced core with a single output gate | Accepted |
| [0008](adr/0008-redundancy-and-failover.md) | Redundancy and failover in three levels | Accepted |
| [0009](adr/0009-multi-user-editing.md) | Multi-user editing, with the server as the single source of truth | Accepted |
| [0010](adr/0010-orbital-remote-nodes.md) | Orbital remote nodes | Accepted (amended by 0025) |
| [0011](adr/0011-license.md) | MIT licence, DCO, trademarked name | Accepted |
| [0012](adr/0012-simulated-devices.md) | Simulated devices built into the SDK | Accepted |
| [0013](adr/0013-plugin-distribution.md) | Plugin registry, signing, offline install, pinning | Accepted |
| [0014](adr/0014-time-sources-and-scheduling.md) | Time sources and scheduling | Proposed |
| [0015](adr/0015-defer-plugin-permission-enforcement.md) | Defer plugin permission enforcement until third-party plugins | Accepted |
| [0016](adr/0016-protocol-schema-tooling.md) | Protocol schema tooling: TypeSpec → JSON Schema | Accepted |
| [0017](adr/0017-wire-paths-as-segment-arrays.md) | State paths on the wire are segment arrays | Accepted |
| [0018](adr/0018-flows-as-step-trees.md) | Flows are structured step trees; bindings are expressions | Accepted |
| [0019](adr/0019-dynamic-action-targets-and-interfaces.md) | Dynamic action targets and plugin interfaces | Proposed (amended by 0026, 0027, 0030, 0032) |
| [0020](adr/0020-project-entity-types-in-the-core.md) | Project entity types live in the core, as serde types | Accepted |
| [0021](adr/0021-expressions-as-sandboxed-snippets.md) | Expressions are sandboxed snippets; what code can do depends on where it runs | Accepted (amended by 0029) |
| [0022](adr/0022-surfaces-pages-and-the-web-surface.md) | Surfaces and pages are core entities; the web surface speaks the surface protocol | Accepted (amended by 0024) |
| [0023](adr/0023-key-rendering-and-binding-evaluation.md) | Keys are rendered in the core; structured bindings are evaluated without QuickJS | Accepted (amended by 0026) |
| [0024](adr/0024-surfaces-as-two-way-devices.md) | Surfaces are two-way devices; control input is device state; layers decide who controls each control | Proposed (amended by 0027, 0029) |
| [0025](adr/0025-device-drivers-are-plugins.md) | Device drivers are plugins; the core performs hardware I/O, and network I/O is the plugin's choice | Proposed (amended by 0029) |
| [0026](adr/0026-sources-and-selectors.md) | One way to give a value, one way to pick things: sources and selectors | Proposed |
| [0027](adr/0027-events-addressed-by-path.md) | Events are addressed by state path; control triggers become event triggers | Proposed |
| [0028](adr/0028-the-core-is-a-connection.md) | The core is a connection; surfaces and flows are its targets | Proposed |
| [0029](adr/0029-dataflow-sources-transforms-sinks.md) | Dataflow is sources, transforms and sinks; helpers are transforms; transforms are sandboxed definitions | Proposed |
| [0030](adr/0030-composite-connections.md) | Composite connections: users build devices out of other devices | Proposed (amended by 0032) |
| [0031](adr/0031-templates-instances-and-components.md) | Reuse is by parameters: templates, instances, components, and explicit scope | Proposed (amended by 0032) |
| [0032](adr/0032-one-template-primitive.md) | One reuse primitive: templates and instances, for layers, composites, components and flows | Proposed |

Each record covers its context, the decision, its consequences and the alternatives considered. Don't edit an accepted record to change its decision; write a new one that supersedes it.
