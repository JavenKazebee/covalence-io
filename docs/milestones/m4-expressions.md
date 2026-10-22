# Milestone 4: Expressions

- **Status:** Planned

Flows start reading data. QuickJS arrives in the core ([ADR 0004](../adr/0004-javascript-expressions-and-scripts.md)), and expressions can appear in action parameters, wait durations, trigger filters and the new steps. A flow can react to a state value changing, not just to a plugin event. It can see what earlier steps returned, branch on it, wait for a condition, and set run-local and user variables. Scripts, loops and the other blocks stay out: M4 is expressions only, with no statements.

## Done when

- [ ] An action step whose parameter is `{expr: "var.nextSource"}` routes `vh` to whatever `var.nextSource` holds when the step starts. If the value fails the action's params schema, the step fails with code `invalidParams` and the evaluated value is in the error.
- [ ] A flow with a `state` trigger `when: "vh.outputs[1].source == 3"`, `edge: rising` runs once each time output 1 changes to source 3, not when it's already 3 at startup and not again while it stays 3. `why` on its action walks back through `trigger.matched` to the `state.patch` that made the expression true.
- [ ] An `event` trigger with `filter: "trigger.payload.output == 1"` runs only for that output. A filter that is false journals nothing. A flow that listens to `vh` `routeChanged` with a filter excluding its own route, and acts on `vh`, doesn't loop, including on a live device's echo with no `cause`.
- [ ] A `branch` step picks the first arm whose `when` is true, or `else`. `steps.<id>.result` from an earlier action is readable in a later step's expression, and steps in arms that didn't run read as `undefined`.
- [ ] A `waitUntil` step with `until: "vh.outputs[1].source == 4"` finishes as soon as that's true (straight away if it already is), and fails with `timeout` if its `timeout` passes first. Cancelling the run during it ends the run `cancelled`.
- [ ] `set` writes `local.<name>` (visible to later steps in the same run, gone after it) and `var.<name>` (a `state.patch` in the journal with `cause` → its `run.step`, visible to bindings and other flows).
- [ ] An expression that throws, loops forever or allocates too much fails its step (or, for a trigger or filter, is treated as false and logged, rate-limited) within the evaluation limits, without stalling the core.
- [ ] The flow editor edits all of the above: any parameter can switch between a literal and an expression, expressions show a live preview against current state, and `branch` arms nest their own step lists.
- [ ] A Rust end-to-end test covers the above in CI, with no hardware. A browser smoke test runs one expression flow through the UI.

## Phases

### Phase 0: Decisions and docs
- [ ] **Where QuickJS lives.** Proposal: a `covalence_core::expr` module wrapping `rquickjs` on QuickJS-ng, behind a small `Evaluator` trait, so the flow runtime and its tests don't depend on the engine directly. One runtime per core, one context per evaluation scope (a run, a trigger), reused rather than rebuilt per evaluation.
- [ ] **Expression-only syntax check.** ADR 0004 parses with `oxc` in the core. ADR 0019 (proposed) would make `oxc` editor-only. M4 needs the core to reject statements, assignments and function definitions, and to find `steps.<id>` references for validation. Decide between `oxc` in the core and a lighter check, and record it.
- [ ] **Dependency tracking.** `state` triggers and `waitUntil` re-evaluate when what they read changes. Proposal: accept the run-time read-set tracking from ADR 0019 now, in a short ADR of its own, and leave dynamic targets and interfaces proposed. Reads go through a state proxy that records paths.
- [ ] **Evaluation limits.** A time limit per evaluation (proposal: 50 ms, via the interrupt hook), a memory limit per runtime, and what each failure looks like (code `exprTimeout`, `exprMemory`, `exprError`, with message and source position).
- [ ] **Determinism.** No `now()` or `random()` in M4 expressions, so nothing needs a `nondet` entry yet. Time is read from `sys.time.now` state. The standard library from ADR 0004 (`mmss`, `clamp`, `round` …) is in.
- [ ] **`var` without declarations.** Proposal: `set` can write any `var.<name>` with any value; the `variable` entity, its schema and persistence across restart wait for the variables milestone. Values in `var` are lost on restart until then.
- [ ] **Trigger and condition journaling.** A false `filter` journals nothing, since it fires on every event. A false flow `condition` journals `trigger.matched` with `blocked: true, reason: condition`, as the spec already says. A `state` trigger's first evaluation sets its baseline and never fires.
- [ ] Update the [flow spec](../specs/flows.md) §8 from "the M3 subset" to the implemented subset, with the M4 additions: `{expr}` params, `wait.for` as an expression, `state` triggers, `event` filters, `conditions`, `branch`, `waitUntil`, `set`, `steps.<id>` and `local`. Add the endpoint for live preview to §9.

### Phase 1: Expression engine (Rust)
- [ ] `rquickjs` on QuickJS-ng in `covalence_core::expr`: compile, evaluate with a scope, return a `Value` or an error with its source position.
- [ ] Value conversion both ways, including `{int}` to a JS number within the safe range, and `undefined` for missing values.
- [ ] Scope objects for `trigger`, `local`, `steps`, and the state roots through a read-recording proxy. Missing paths read as `undefined`, never throw.
- [ ] The expression-only check chosen in Phase 0, and extraction of `steps.<id>` references.
- [ ] Time and memory limits; cancellation through the interrupt hook.
- [ ] The standard library.
- [ ] Unit tests for the language edges: `==` across int and float, missing paths, bracket keys (`vh.outputs["SDI 4"]`), limits, cancellation. A benchmark for one simple expression against the 16 ms re-render budget in [architecture](../architecture.md#performance-budgets-targets-to-test-against).

### Phase 2: Flow model and runtime (Rust)
- [ ] Flow types gain `{expr}` params, `wait.for` as an expression, `state` triggers, `event` `filter`, flow `conditions`, and the `branch`, `waitUntil` and `set` steps. `Flow::parse` still rejects everything else with its path.
- [ ] Validation: expression syntax, references to step IDs that don't exist or come later, `set` paths that aren't `local.<name>` or `var.<name>`, duplicate step IDs across nested arms. Literal params are still checked against the schema; expression params are checked at run time.
- [ ] Fixtures in `crates/covalence-core/fixtures/flows/` for each new shape, and `problems/` for each new error.
- [ ] `steps.<id>` results `{status, result?, error?, startedAt, durationMs}` kept per run. `local` kept per run.
- [ ] `branch`: positions become nested (`[branch1, route2]`). `run.step` entries for the branch itself and for its chosen arm's steps.
- [ ] `waitUntil` subscribes to its read set, re-evaluates on change and wakes on cancel or timeout.
- [ ] `set` on `var` goes through the state store as a `state.patch` with origin `{flow, run, step}` and `cause` → its `run.step`.
- [ ] Trigger matcher for `state` triggers: evaluate on load and whenever the read set changes, fire on the chosen edge, `cause` → the `state.patch` entry. Filters and conditions are evaluated with `trigger` in scope before the concurrency mode is applied.
- [ ] The loop guard (§8.6) also covers `state` triggers.

### Phase 3: HTTP API
- [ ] The flow endpoints accept and return the new shapes; validation problems keep `{path, code, message}`, with expression errors pointing into the source.
- [ ] `POST /expressions/evaluate {source, flow?}`: evaluates against live state for the editor's preview and returns `{value}` or `{error}`. It never writes and never runs actions.
- [ ] End-to-end tests for every "Done when" item the API can reach, in `crates/covalence-server/tests/`.

### Phase 4: UI
- [ ] Expression input: CodeMirror 6 with JS highlighting, inline errors from the server, and a debounced live preview from `POST /expressions/evaluate`.
- [ ] `SchemaForm` fields switch between literal and expression. `wait` takes a duration or an expression.
- [ ] Triggers: a `state` trigger editor (`when`, `edge`), a `filter` on `event` triggers, and flow conditions.
- [ ] Step editors for `branch` (arms with `when` and nested step lists, plus `else`), `waitUntil` and `set`. Reordering works within a list; moving steps between arms can wait.
- [ ] The runs view highlights nested positions. The `why` drawer shows `state.patch` causes and `set` writes readably.
- [ ] `src/flows/model.ts` gains the new types, tested against the shared fixtures: every valid fixture round-trips through the editor unchanged.

### Phase 5: Acceptance
- [ ] The Rust end-to-end tests run in the existing `cargo test` CI job.
- [ ] A browser smoke test in `packages/ui/e2e/`: create a flow with an expression parameter and a `branch`, run it, and see the route change. This also closes M3's gap of nothing in CI checking the Flows UI and API together.
- [ ] Mark the live-echo loop gap in [M1 known gaps](m1-walking-skeleton.md#known-gaps-carried-forward) and flow spec §8.6 as fixed by filters, or say what's left.

## Ordering

Phase 0 comes first. Phase 1 can start once the engine and syntax-check decisions are made. Phase 2 needs Phase 1's evaluator trait, not its finished engine: it can run against a stub. Phase 3 follows Phase 2. Phase 4 can start against the written-down shapes and a stubbed preview endpoint. Phase 5 joins them.

## Out of scope

Scripts and the `script` step · `now()` and `random()` and `nondet` entries · `group`, `loop` and `parallel` blocks, `call`, `log` and `stop` steps · `schedule`, `gesture`, `call`, `osc` and `startup` triggers · flow `inputs` and `outputs` · `retry` · dynamic targets and interfaces ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) · generated TypeScript definitions, type checking of expressions and editor autocomplete · the `variable` entity and persisted variables · expression bindings on surfaces · checkpoints and failover policies · `action.cancel`.
