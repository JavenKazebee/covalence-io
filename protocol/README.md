# Protocol schema

The single source of truth for the plugin protocol ([plugin protocol §13.1](../docs/specs/plugin-protocol.md#131-protocol-first), [plugin protocol §13.2](../docs/specs/plugin-protocol.md#132-schema-tooling-typespec--json-schema)).

| Path | Contents |
|---|---|
| `main.tsp`, `src/*.tsp` | TypeSpec source |
| `json-schema/` | Emitted JSON Schema. Generated, but committed, so other-language SDKs don't need TypeSpec |
| `scripts/gen-ts.mjs` | Generates `packages/protocol/types.gen.ts` from the JSON Schema |
| `interfaces/*.json` | The core's interfaces ([plugin protocol spec §13.9](../docs/specs/plugin-protocol.md#139-dynamic-targets-proposed)): `surface@1` and `flow@1`, which the core connection implements (core actions spec) |
| `fixtures/messages.json` | Golden messages: every message type, with every optional field filled |
| `fixtures/invalid.json` | Messages both sides must reject |
| `fixtures/encoded/` | MessagePack frames produced by each side; each side's tests decode the other's |

Rust types in `crates/covalence-protocol` are written by hand ([plugin protocol spec §13.2](../docs/specs/plugin-protocol.md#132-schema-tooling-typespec--json-schema) fallback). The fixtures keep them in step with the schema.

## Changing the protocol

1. Edit the `.tsp` files, then regenerate:
   ```bash
   pnpm --filter @covalence-io/protocol-schema build
   ```
2. Mirror the change in `crates/covalence-protocol`.
3. Add or update fixtures, then regenerate both sides' frames:
   ```bash
   COVALENCE_UPDATE_FIXTURES=1 cargo test -p covalence-protocol --test fixtures
   COVALENCE_UPDATE_FIXTURES=1 deno task test
   ```
4. Run `cargo test` and `deno task test` again without the flag. Both must pass.
