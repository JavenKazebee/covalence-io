# Flow fixtures

Example flows shared by the Rust tests (`crates/covalence-core/tests/flow_fixtures.rs`) and the UI's tests ([overview spec: project entities](../../../../docs/specs/overview.md#project-entities-are-rust-types)).

- `*.json`: valid flows, exactly as stored and as the API sends them. Each must parse, validate with no errors or warnings, and round-trip unchanged.
- `instances/*.json`: instances of `template-route.json` (composition spec §9), with only their identity, `params` bindings and `forEach`. The UI has no editor for them yet, so they aren't round-tripped through it.
- `problems/*.json`: `{flow, errors, warnings}`, where `errors` and `warnings` list the expected `{path, code}` of each problem, in the order validation reports them.

The connections they refer to:

| ID | Alias | Plugin |
|---|---|---|
| `01JBZ7P0D3F5G7H9J1K3L5M7N9` | `vh` | Videohub |
| `01JBZ7P0D3F5G7H9J1K3L5M7NA` | `vh2` | Videohub |
| `01JBZ7P0D3F5G7H9J1K3L5M7NB` | `later` | A plugin that hasn't loaded |

The layer they refer to:

| ID | Controls |
|---|---|
| `01JBZ7P0D3F5G7H9J1K3L5M7PA` | `k0` to `k14`, each `momentary` with an `image` output, as the virtual deck has |

The flows they call:

| ID | Name | Triggers | Inputs | Outputs |
|---|---|---|---|---|
| `01JC00000000000000000000CA` | Recall | `call` | `camera` (integer ≥ 1, required), `speed` (number, default 1) | `preset` (integer) |
| `01JC00000000000000000000CB` | The same, with only an `api` trigger | `api` | as above | as above |

The template the instances use is `template-route.json` (`01JC00000000000000000000TA`): params `hub` (a Videohub, required), `output` (integer, default 1) and `then` (a flow, default Recall).

The slug `taken` belongs to another flow.
