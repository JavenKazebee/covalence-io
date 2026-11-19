# Binding fixtures

Example control bindings shared by the Rust tests (`crates/covalence-core/tests/bindings.rs`) and the UI's tests ([overview spec: project entities](../../../../docs/specs/overview.md#project-entities-are-rust-types)).

- `*.json`: valid bindings, exactly as a component stores them and the API sends them ([composition spec §7.1](../../../../docs/specs/composition.md#71-controls-and-bindings)). Each must parse, validate with no errors, round-trip unchanged, and give the same result evaluated natively and as its compiled expression.
- `problems/*.json`: `{binding, errors}`, where `errors` lists the expected `{path, code}` of each problem, in the order validation reports them, relative to the binding.

Each is checked as a binding in the 15-key component `01JBZ7P0D3F5G7H9J1K3L5M7PA` of the [flow fixtures](../flows/README.md), with the connection `vh` (`01JBZ7P0D3F5G7H9J1K3L5M7N9`). Control `k14` in that component is bound by another binding, and `k13` there lists no outputs.
