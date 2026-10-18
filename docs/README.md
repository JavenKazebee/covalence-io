# Covalence IO documentation

## Start here
- [Architecture overview](architecture.md): components, core concepts and repo layout.

## Milestones
- [M1: Walking skeleton](milestones/m1-walking-skeleton.md) (in progress)

## Specs (drafts)
- [State model and event log](specs/state-and-event-log.md)
- [Plugin protocol](specs/plugin-protocol.md)

## Architecture decision records

| # | Decision | Status |
|---|---|---|
| [0001](adr/0001-rust-core.md) | Core engine in Rust | Accepted |
| [0002](adr/0002-protocol-first-plugins.md) | Plugins defined protocol-first | Accepted |
| [0003](adr/0003-plugin-runtime-deno.md) | Plugins share one Deno runtime, with per-connection isolation | Accepted (spike deferred by 0015) |
| [0004](adr/0004-javascript-expressions-and-scripts.md) | JS/TS for expressions and scripts (QuickJS in the core) | Accepted |
| [0005](adr/0005-web-ui-vue-tauri-shell.md) | Vue 3 web UI served by the core, with an optional Tauri shell | Accepted |
| [0006](adr/0006-project-storage-and-versioning.md) | SQLite working store, text export, migrations | Accepted |
| [0007](adr/0007-event-sourced-core.md) | Event-sourced core with a single output gate | Accepted |
| [0008](adr/0008-redundancy-and-failover.md) | Redundancy and failover in three levels | Accepted |
| [0009](adr/0009-multi-user-editing.md) | Multi-user editing, with the server as the single source of truth | Accepted |
| [0010](adr/0010-orbital-remote-nodes.md) | Orbital remote nodes | Accepted |
| [0011](adr/0011-license.md) | MIT licence, DCO, trademarked name | Accepted |
| [0012](adr/0012-simulated-devices.md) | Simulated devices built into the SDK | Accepted |
| [0013](adr/0013-plugin-distribution.md) | Plugin registry, signing, offline install, pinning | Accepted |
| [0014](adr/0014-time-sources-and-scheduling.md) | Time sources and scheduling | Proposed |
| [0015](adr/0015-defer-plugin-permission-enforcement.md) | Defer plugin permission enforcement until third-party plugins | Accepted |
| [0016](adr/0016-protocol-schema-tooling.md) | Protocol schema tooling: TypeSpec → JSON Schema | Accepted |
| [0017](adr/0017-wire-paths-as-segment-arrays.md) | State paths on the wire are segment arrays | Accepted |

Each record covers its context, the decision, its consequences and the alternatives considered. Don't edit an accepted record to change its decision; write a new one that supersedes it.
