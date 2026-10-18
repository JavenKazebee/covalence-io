# Protocol schema

The single source of truth for the plugin protocol ([ADR 0002](../docs/adr/0002-protocol-first-plugins.md), [ADR 0016](../docs/adr/0016-protocol-schema-tooling.md)).

| Path | Contents |
|---|---|
| `main.tsp`, `src/*.tsp` | TypeSpec source |
| `json-schema/` | Emitted JSON Schema. Generated, but committed, so other-language SDKs don't need TypeSpec |
| `scripts/gen-ts.mjs` | Generates `packages/protocol/types.gen.ts` from the JSON Schema |
| `fixtures/messages.json` | Golden messages: every message type, with every optional field filled |
| `fixtures/invalid.json` | Messages both sides must reject |
| `fixtures/encoded/` | MessagePack frames produced by each side; each side's tests decode the other's |

Rust types in `crates/covalence-protocol` are written by hand (ADR 0016 fallback). The fixtures keep them in step with the schema.

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
