# 0016 – Protocol schema tooling: TypeSpec → JSON Schema

- **Status:** Accepted
- **Date:** 2026-10-18
- **Resolves:** the schema-tool choice left Proposed in [0002](0002-protocol-first-plugins.md), and [plugin protocol](../specs/plugin-protocol.md) open question 1

## Context
- [0002](0002-protocol-first-plugins.md) makes the schema in `protocol/` the single source of truth for Rust and TypeScript types.
- Writing JSON Schema by hand is verbose and hard to review. The protocol has many message shapes and discriminated unions.
- `typify` (JSON Schema → Rust) handles plain objects well but has historically been less reliable on `oneOf` unions such as the message envelope.

## Decision
- **Write the schema in TypeSpec** in `protocol/`, and emit JSON Schema with `@typespec/json-schema`. The emitted JSON Schema is committed, so other-language SDKs never need TypeSpec.
- **TypeScript types:** generated from the JSON Schema with `json-schema-to-typescript`, into `packages/protocol`.
- **Rust types:** generated with `typify`, into `covalence-protocol`.
- **Fallback, decided on the first day of the schema work:** if `typify` output for the envelope or other unions is wrong or unusable, write the Rust `serde` types by hand. Golden vectors in `protocol/fixtures/` are then the contract between the two sides.
- **Golden vectors are required either way.** Every message type has fixtures that both the Rust and TypeScript test suites decode, re-encode and compare.
- Opaque payloads that are themselves JSON Schema (`config.schema`, action `params`) are typed as plain JSON values, not modelled in TypeSpec.

## Outcome of the first-day test (2026-10-18)
The fallback applies: **Rust types are hand-written**, TypeScript types are generated.

With `typify` 0.8.0 (the latest stable release) on the envelope from the [plugin protocol](../specs/plugin-protocol.md) §2.1:
- `const` was dropped: `t: "req"` became `t: String` and `ok: true` became `ok: bool`, so the discriminator wasn't enforced.
- The union became `#[serde(untagged)]`, so decoding tries each variant in turn. A notification carrying a stray `id` decoded as a request.
- Rewriting the schema to `oneOf` with single-value `enum` tags enforced the tags, but the enum stayed untagged, and it required changing the wire format to suit the tool.
- Opaque payloads became `serde_json::Value`, which can't hold MessagePack binary (needed later for `surface.draw` and `io.data`).

`json-schema-to-typescript` produced clean output: literal types and a proper discriminated union.

## Consequences
- The schema is readable and reviewable, and generated types can't drift from it.
- A Node toolchain (pnpm) is needed to change the schema, but not to build the core or write plugins.
- If the fallback is used, the Rust types can drift from the schema; the golden vectors catch that in CI.

## Alternatives considered
- **Hand-written JSON Schema:** no extra tool, but verbose and error-prone.
- **Rust-first (`serde` + `schemars`):** convenient for the core, but makes Rust the source of truth, which [0002](0002-protocol-first-plugins.md) rules out.
- **TypeScript-first (zod or similar):** same problem, the other way round.
- **Protobuf:** ruled out in [0002](0002-protocol-first-plugins.md).
