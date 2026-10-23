# Binding fixtures

Example control bindings shared by the Rust tests (`crates/covalence-core/tests/bindings.rs`) and, from M5 Phase 4, the UI's tests (ADR 0020).

- `*.json`: valid bindings, exactly as stored and as the API sends them. Each must parse, validate with no errors, round-trip unchanged, and give the same result evaluated natively and as its compiled expression.
- `problems/*.json`: `{binding, errors}`, where `errors` lists the expected `{path, code}` of each problem, in the order validation reports them.

They use the connection and layer of the [flow fixtures](../flows/README.md): `vh` (`01JBZ7P0D3F5G7H9J1K3L5M7N9`) and the 15-key layer `01JBZ7P0D3F5G7H9J1K3L5M7PA`. Control `k14` on that layer is bound by another binding, and `k13` there lists no outputs.
