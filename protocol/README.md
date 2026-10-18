# Protocol schema

The single source of truth for the plugin protocol ([ADR 0002](../docs/adr/0002-protocol-first-plugins.md), [ADR 0016](../docs/adr/0016-protocol-schema-tooling.md)).

- `*.tsp`: TypeSpec source.
- `json-schema/`: emitted JSON Schema, committed so other-language SDKs don't need TypeSpec.
- `fixtures/`: golden message vectors that the Rust and TypeScript test suites both round-trip.

Generated types land in `crates/covalence-protocol` and `packages/protocol`.
