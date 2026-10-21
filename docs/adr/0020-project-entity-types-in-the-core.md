# 0020 – Project entity types live in the core, as serde types

- **Status:** Accepted
- **Date:** 2026-10-21
- **Specs:** [flows](../specs/flows.md), [state and event log](../specs/state-and-event-log.md)
- **Related:** [ADR 0006](0006-project-storage-and-versioning.md), [ADR 0016](0016-protocol-schema-tooling.md)

## Context
- Flows are the first project entity with a deep structure. Connections came first, but they're a few flat fields.
- [ADR 0016](0016-protocol-schema-tooling.md) put the **plugin protocol** in TypeSpec, with JSON Schema and TS types generated from it. The Rust side ended up hand-written anyway, because `typify` failed the day-one test, so TypeSpec only produces TS for the plugin side.
- The plugin protocol is a contract with code we don't own: third-party plugins, written against a published schema. Project entities are a contract between the core and our own UI, and with the exported YAML ([ADR 0006](0006-project-storage-and-versioning.md)).
- The UI already mirrors the HTTP API by hand in `packages/ui/src/api.ts`. That has worked for M2, but a flow has nested unions (triggers, steps), where silent drift is easier.
- More entities follow the same path: surfaces, pages, bindings, variables. Whatever flows do sets the pattern.

## Decision
- **Project entity types are Rust serde types in `covalence-core`**, one module per entity (`covalence_core::flow`). They are the source of truth for storage, the HTTP API and validation.
- **Field names follow the specs,** which are written in TypeScript notation: camelCase, with the same names the YAML export will use. The JSON stored in the `entities` table is the API's JSON.
- **The UI mirrors them by hand** in TypeScript, as it does the rest of the API.
- **Shared fixtures guard against drift.** Example entities live as JSON files in `crates/covalence-core/fixtures/<entity>/`. Rust tests check that each one parses and round-trips unchanged. UI tests check that each one type-checks against the TS types and that the editor round-trips it. A fixture that should fail validation sits beside the valid ones with the expected error codes.
- **TypeSpec stays for the plugin protocol only.** If the editor later needs a machine-readable flow schema (for the TypeScript definitions in flow spec §4, say), generate JSON Schema from the Rust types with `schemars` rather than adding a second source.

## Consequences
- One source of truth per contract: TypeSpec for plugins, Rust for the project.
- Adding a field means editing a Rust type, a TS type and usually a fixture. The fixtures make a missed TS edit fail a test rather than a show.
- The core validates entities with full access to manifests and state, which validation needs anyway (unknown action, parameter types).
- No code generation step in the UI build.

## Alternatives considered
- **TypeSpec for project entities too:** one schema language across the repo. But TypeSpec's TS output is all it would buy us, Rust would still be hand-written, and validation needs runtime context (manifests) that a schema can't express.
- **Generate TS from Rust** (`ts-rs`, `specta`): no hand-mirroring. It adds a build step that has to run before the UI type-checks, and the generated unions are awkward to use in Vue forms. Worth revisiting if the hand-written mirror starts to drift despite the fixtures.
- **JSON Schema as the source, Rust and TS generated:** the `typify` result from ADR 0016 already rules out the Rust half.
