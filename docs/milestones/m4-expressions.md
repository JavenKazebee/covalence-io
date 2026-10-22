# Milestone 4: Expressions

- **Status:** In progress (Phases 0–3 done)
- **Started:** 2026-10-21

Flows start reading data. QuickJS arrives in the core ([ADR 0004](../adr/0004-javascript-expressions-and-scripts.md)), and expressions can appear in action parameters, wait durations, trigger filters and the new steps. A flow can react to a state value changing, not just to a plugin event. It can see what earlier steps returned, branch on it, wait for a condition, and set run-local and user variables. Expressions can use any JavaScript syntax but can't change anything ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)). Script steps, which can act, stay out, as do loops and the other blocks.

## Done when

- [x] An action step whose parameter is `{expr: "vars.nextSource"}` routes `vh` to whatever `vars.nextSource` holds when the step starts. If the value fails the action's params schema, the step fails with code `invalidParams` and the evaluated value is in the error.
- [x] A flow with a `state` trigger `when: "vh.outputs[1].source == 3"`, `edge: rising` runs once each time output 1 changes to source 3, not when it's already 3 at startup and not again while it stays 3. `why` on its action walks back through `trigger.matched` to the `state.patch` that made the expression true.
- [x] An `event` trigger with `filter: "trigger.payload.output == 1"` runs only for that output. A filter that is false journals nothing. A flow that listens to `vh` `routeChanged` with a filter excluding its own route, and acts on `vh`, doesn't loop, including on a live device's echo with no `cause`.
- [x] A `branch` step picks the first arm whose `when` is true, or `else`. `steps.<id>.result` from an earlier action is readable in a later step's expression, and steps in arms that didn't run read as `undefined`.
- [x] A `waitUntil` step with `until: "vh.outputs[1].source == 4"` finishes as soon as that's true (straight away if it already is), and fails with `timeout` if its `timeout` passes first. Cancelling the run during it ends the run `cancelled`.
- [x] `set` writes `local.<name>` (visible to later steps in the same run, gone after it) and `vars.<name>` (a `state.patch` in the journal with `cause` → its `run.step`, visible to bindings and other flows).
- [x] A multi-line expression with a `const` lookup table and a `return` works as an action parameter. An expression that writes (`vars.x = 1`), calls `Date.now()` or `Math.random()`, or returns a promise fails with a clear error, and nothing it touched changes.
- [x] An expression that throws, loops forever or allocates too much fails its step (or, for a trigger, filter or condition, counts as false and is logged, rate-limited) within the evaluation limits, without stalling the core.
- [ ] The flow editor edits all of the above: any parameter can switch between a literal and an expression, expressions show a live preview against current state, and `branch` arms nest their own step lists.
- [ ] A Rust end-to-end test covers the above in CI, with no hardware. A browser smoke test runs one expression flow through the UI.

## Phases

### Phase 0: Decisions and docs
- [x] **Language.** JavaScript stays, on QuickJS-ng via `rquickjs`. Boa, V8 and running in the Deno host were compared as engines; Rhai, Rune, Lua/Luau, CEL and Starlark as languages ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)).
- [x] **Expressions are sandboxed snippets, not expression syntax** ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)). Any syntax, compiled as an expression first and then as a function body. Purity comes from read-only scope, no host API, a fresh or frozen context, no `Date.now()` or `Math.random()`, and no async. Scripts share the language and engine and differ only in what they can do.
- [x] **No `oxc` in the core in M4.** QuickJS reports syntax errors. Unknown `steps.<id>` references are editor warnings. `oxc` returns with script steps, for TypeScript.
- [x] **Dependency tracking at run time,** with read sets recorded through the state proxy, taken over from ADR 0019 into ADR 0021. Dynamic targets and interfaces stay proposed.
- [x] **Where QuickJS lives:** a `covalence_core::expr` module behind an `Evaluator` trait, so the flow runtime and its tests don't depend on the engine directly.
- [x] **Evaluation limits:** 50 ms per evaluation through the interrupt hook, and 16 MB of memory, as core settings. Failures are `exprError`, `exprTimeout` and `exprMemory`, with message and source position.
- [x] **Determinism.** No time or randomness in expressions, so nothing needs a `nondet` entry yet. Time is read from `sys.time.now` state. The standard library from ADR 0004 (`mmss`, `clamp`, `round` …) is in.
- [x] **`vars` without declarations.** `set` can write any `vars.<name>` with any value. The `variable` entity, its schema and persistence across restarts wait for a variables milestone, so `vars` values are lost on restart until then.
- [x] **Trigger and condition journaling.** A false `filter` journals nothing, since it runs on every event. A false flow `condition` journals `trigger.matched` with `blocked: true, reason: condition`. A `state` trigger's first evaluation sets its baseline and never fires.
- [x] [Flow spec](../specs/flows.md) §8 is now "the implemented subset", with the M4 additions marked: `{expr}` params, `wait.for` as an expression, `state` triggers, `event` filters, `conditions`, `branch`, `waitUntil`, `set`, `steps.<id>` and `local`, the matching order, and a new §8.7 on expressions. §9 has `POST /expressions/evaluate`.

### Phase 1: Expression engine (Rust)
- [x] `rquickjs` 0.14 on QuickJS-ng in `covalence_core::expr` (`QuickJs`, behind the `Evaluator` trait): compile as an expression, else as a function body; evaluate with a `Scope`; return a `Value` or an `ExprError` with its source position. `check` compiles without running, for validation.
- [x] Value conversion both ways, through JSON. Ints within ±2⁵³ stay ints; `undefined` is "no value"; NaN, infinities, functions, symbols and promises are errors.
- [x] Scope objects for `trigger`, `local`, `steps`, and the state roots through read-only, read-recording proxies (`prelude.js`). Missing paths read as `undefined`, never throw. Writes throw. Listing a node's keys reads its whole subtree. `Reads::touched_by` matches a dirty set.
- [x] The sandbox: no host API, and `Date.now()`, `Date()`, argument-less `new Date()` and `Math.random()` throw. `Promise` is hidden, and there are no timers, `performance` or weak references. **A fresh context per evaluation,** measured at ~265 µs in release, ~190 µs of it parsing the prelude. A frozen, reused context wasn't built: it needs a lockdown of every built-in to be safe. Loading the prelude as bytecode measured ~95 µs, but needs `unsafe`; deferred until surface bindings need the speed.
- [x] Time and memory limits (50 ms, 16 MB); cancellation through the interrupt hook, from the run's cancel channel. Both are uncatchable. Deep recursion fails with `exprError`.
- [x] The standard library: `mmss`, `hhmmss`, `clamp`, `round`, `pad`, `lerp`, `rgb`, `hsl`, `mix`. Colours are `#rrggbb` strings.
- [x] Unit tests in `crates/covalence-core/src/expr/tests.rs` for the language edges, every sandbox escape, read tracking, limits and cancellation. The benchmark is an ignored test: `cargo test --release -p covalence-core --lib expr::tests::bench -- --ignored --nocapture`.
- [x] **`var` became `vars`** everywhere, because `var` is a JavaScript keyword ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)).
- Carried to Phase 2: aliases that are JavaScript keywords or global names (`if`, `Math`, `clamp`, `steps`) can't be read in expressions. The engine leaves such names unbound rather than hiding a global; alias validation in the server should reject them.

### Phase 2: Flow model and runtime (Rust)
- [x] Flow types gain `{expr}` params, `wait.for` as an expression (`WaitFor`), `state` triggers (`Edge`), `event` `filter`, flow `conditions`, and the `branch`, `waitUntil` and `set` steps. `Flow::parse` still rejects everything else with its path, including inside `branch` arms.
- [x] Validation: expressions are compiled through the `Evaluator` in `flow::Context` (`exprSyntax`, with `at`), `set` paths (`badPath`), branches without arms (`noArms`), step IDs unique across nested arms. Literal params are checked against the schema; expression params are skipped. A `waitUntil` without a `timeout` is a warning (`noTimeout`).
- [x] Fixtures: `expressions.json` uses every new shape; `problems/expressions.json` has each new error; `problems/unsupported.json` now covers what's still later, including nested in a branch.
- [x] `steps.<id>` results `{status, result?, error?, startedAt, durationMs}` and `local`, kept per run.
- [x] `branch`: nested positions (`[pick, two]`), and the branch's `done` entry carries `arm`.
- [x] `waitUntil` subscribes to the state feed, re-evaluates when its read set changes, and wakes on cancel or timeout.
- [x] `set` on `vars` journals a `state.patch` with origin `{flow, run, step}` and `cause` → its `run.step`, then applies it.
- [x] `state` triggers: a watcher task re-evaluates them once per batch of state changes that touches their read set, and fires on the edge, `cause` → the latest `state.patch` it read. Saving a flow sets a baseline that never fires. Filters and conditions are evaluated outside the runtime's lock, before the concurrency mode.
- [x] The loop guard (§8.6) covers `state` triggers, including a flow that writes what it watches.
- [x] Expression params are checked against the action's schema before the request (`invalidParams`), through a new `Actions::params_schema`.
- [x] Aliases that are JavaScript keywords or names expressions already have (`var`, `if`, `Math`, `clamp`, `steps`) are rejected when a connection is created (`expr::is_taken_name`). The runtime gets aliases from the server through `Runtime::set_aliases`.
- [x] Tests: `crates/covalence-core/tests/flow_runtime.rs` covers expression params, step results, `branch`, conditions, filters, `state` edges and their cause, the loop guard, `waitUntil` and cancelling it.
- Known gap: a `timeout` on a `branch` stops the block by dropping it, so a step running inside it is journaled `started` with no end. Fix when blocks get proper cancellation (`group`, `loop`).
- Known gap: evaluations run on the async worker that needs them and share one engine, so a slow expression (up to 50 ms) delays others. Fine for flows; revisit with bindings.

### Phase 3: HTTP API
- [x] The flow endpoints accept and return the new shapes (through `save_flow`); validation problems keep `{path, code, message}`, and expression errors add `at` within the source.
- [x] `POST /expressions/evaluate {source, trigger?}` (`crates/covalence-server/src/flows.rs`): evaluates against live state for the editor's preview, on a blocking thread, and returns `{value?, error?, reads}` (flow spec §9). It never writes, runs actions or journals.
- [x] `GET /state` and the WebSocket show the `vars` root beside connections and `flow`.
- [x] End-to-end tests in `crates/covalence-server/tests/flows_api.rs` against the real host and two simulated Videohubs: the preview endpoint, expression params routing to a `set` variable, `invalidParams` before the request, a `state` trigger firing once per rising edge with `why` reaching its `state.patch`, and a filter that keeps a flow acting on its own trigger's connection from looping. Branches, `waitUntil`, cancellation and limits are covered in `crates/covalence-core/tests/flow_runtime.rs` and `expr/tests.rs`.

### Phase 4: UI
- [ ] Expression input: CodeMirror 6 with JS highlighting, a one-line field that grows to several lines, inline errors from the server, and a debounced live preview from `POST /expressions/evaluate`.
- [ ] Editor warnings for `steps.<id>` references to steps that don't exist or come later, found with CodeMirror's JavaScript syntax tree.
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

Scripts and the `script` step · `oxc` and TypeScript in code fields · `now()` and `random()` and `nondet` entries · `group`, `loop` and `parallel` blocks, `call`, `log` and `stop` steps · `schedule`, `gesture`, `call`, `osc` and `startup` triggers · flow `inputs` and `outputs` · `retry` · dynamic targets and interfaces ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) · generated TypeScript definitions, type checking of expressions and editor autocomplete · the `variable` entity and persisted variables · expression bindings on surfaces · checkpoints and failover policies · `action.cancel`.
