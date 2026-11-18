# Milestone 4: Expressions

- **Status:** Done
- **Started:** 2026-10-21

Flows start reading data. QuickJS arrives in the core ([flows §11.2](../specs/flows.md#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)), and expressions can appear in action parameters, wait durations, trigger filters and the new steps. A flow can react to a state value changing, not just to a plugin event. It can see what earlier steps returned, branch on it, wait for a condition, and set run-local and user variables. Expressions can use any JavaScript syntax but can't change anything ([flows §11.2](../specs/flows.md#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)). Script steps, which can act, stay out, as do loops and the other blocks.

## Done when

- [x] An action step whose parameter is `{expr: "vars.nextSource"}` routes `vh` to whatever `vars.nextSource` holds when the step starts. If the value fails the action's params schema, the step fails with code `invalidParams` and the evaluated value is in the error.
- [x] A flow with a `state` trigger `when: "vh.outputs[1].source == 3"`, `edge: rising` runs once each time output 1 changes to source 3, not when it's already 3 at startup and not again while it stays 3. `why` on its action walks back through `trigger.matched` to the `state.patch` that made the expression true.
- [x] An `event` trigger with `filter: "trigger.payload.output == 1"` runs only for that output. A filter that is false journals nothing. A flow that listens to `vh` `routeChanged` with a filter excluding its own route, and acts on `vh`, doesn't loop, including on a live device's echo with no `cause`.
- [x] A `branch` step picks the first arm whose `when` is true, or `else`. `steps.<id>.result` from an earlier action is readable in a later step's expression, and steps in arms that didn't run read as `undefined`.
- [x] A `waitUntil` step with `until: "vh.outputs[1].source == 4"` finishes as soon as that's true (straight away if it already is), and fails with `timeout` if its `timeout` passes first. Cancelling the run during it ends the run `cancelled`.
- [x] `set` writes `local.<name>` (visible to later steps in the same run, gone after it) and `vars.<name>` (a `state.patch` in the journal with `cause` → its `run.step`, visible to bindings and other flows).
- [x] A multi-line expression with a `const` lookup table and a `return` works as an action parameter. An expression that writes (`vars.x = 1`), calls `Date.now()` or `Math.random()`, or returns a promise fails with a clear error, and nothing it touched changes.
- [x] An expression that throws, loops forever or allocates too much fails its step (or, for a trigger, filter or condition, counts as false and is logged, rate-limited) within the evaluation limits, without stalling the core.
- [x] The flow editor edits all of the above: any parameter can switch between a literal and an expression, expressions show a live preview against current state, and `branch` arms nest their own step lists.
- [x] A Rust end-to-end test covers the above in CI, with no hardware (`crates/covalence-server/tests/flows_api.rs`, `crates/covalence-core/tests/flow_runtime.rs`). The browser smoke test was dropped from M4 (Phase 5).

## Known gaps, carried forward

- A `timeout` on a `branch` stops the block by dropping it, so a step running inside it is journaled `started` with no end. Fix when blocks get proper cancellation (`group`, `loop`).

## History

The phase-by-phase build log was trimmed once the milestone closed. It's in git: `git log -p -- docs/milestones/m4-expressions.md`.
