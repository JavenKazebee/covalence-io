# Flow fixtures

Example flows shared by the Rust tests (`crates/covalence-core/tests/flow_fixtures.rs`) and the UI's tests (ADR 0020).

- `*.json`: valid flows, exactly as stored and as the API sends them. Each must parse, validate with no errors or warnings, and round-trip unchanged.
- `problems/*.json`: `{flow, errors, warnings}`, where `errors` and `warnings` list the expected `{path, code}` of each problem, in the order validation reports them.

The connections they refer to:

| ID | Alias | Plugin |
|---|---|---|
| `01JBZ7P0D3F5G7H9J1K3L5M7N9` | `vh` | Videohub |
| `01JBZ7P0D3F5G7H9J1K3L5M7NA` | `vh2` | Videohub |
| `01JBZ7P0D3F5G7H9J1K3L5M7NB` | `later` | A plugin that hasn't loaded |

The slug `taken` belongs to another flow.
