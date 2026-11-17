# Built-in transform definitions

Each file is a transform's definition ([dataflow spec §4.4](../../../docs/specs/dataflow.md)):
a pure JavaScript module whose default export is `defineTransform({...})`. The
definition is the specification. Where the core has a native implementation
(`src/transform/reducer.rs`, or `Transform::evaluate` for the stateless ones),
the conformance suite in `conformance/` runs both over the same inputs and
requires equal states, values and events.

- **Reducers** have `init({options, now})`, `step(state, ctx)` and optionally
  `initValue({state, options})` (the value at `init`; the state when absent)
  and `follow(state, ctx)` (the value a `follow` source sets; the followed
  value as both state and value when absent).
- **`ctx`** is `{event, input, previous, value, followed, options, now}`:
  `event` is `{name, payload}` or `undefined`, `input` and `previous` are the
  input's value now and at the last step, `value` is the cell's current value,
  and `now` is scheduler time in milliseconds.
- **`step` returns** `{state, value, events?, wakeAt?}`, or `undefined` for no
  change. An event is a name or `{name, payload}`.
- **Stateless** definitions have `compute({options})`, with every option
  already evaluated.
- **Options** are filled with their `default` before any call.
