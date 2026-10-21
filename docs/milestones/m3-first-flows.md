# Milestone 3: First flows

- **Status:** In progress
- **Started:** 2026-10-21

The first automation. A flow is a trigger plus a flat list of `action` and `wait` steps, which is the Button-view shape from the [flow spec](../specs/flows.md) §6. A flow can be started from the API or the UI, or by a plugin event. Every run is journaled step by step, so `why` walks from a device action back through the run to whatever started it. QuickJS stays out: parameters are literal values and triggers have no filters or conditions.

## Done when

- [ ] A flow can be created, listed, updated and deleted over `POST/GET/PUT/DELETE /flows`, stored as a `flow` entity, and it survives a server restart.
- [ ] `POST /flows/{id}/run` on a flow with steps `route (vh 1←3)`, `wait 1s`, `route (vh 1←4)` journals `run.started` → `run.step` started/done for each step → `run.finished ok`. `vh.outputs[1].source` reads 3, then 4.
- [ ] A flow with an `event` trigger on `vh` `routeChanged` that routes `vh2` runs whenever `vh` changes. `GET /journal/{seq}/why` on its `vh2` `action.result` walks back through `run.step` → `run.started` → `trigger.matched` → `event` → `vh`'s `action.requested`.
- [ ] All four concurrency modes behave as the spec says: `parallel` starts a second run, `queue` runs it afterwards, `restart` cancels the active run first, and `ignore` journals `trigger.matched` with `blocked: true`.
- [ ] `POST /runs/{run}/cancel` stops a run during a wait, which journals `run.finished cancelled`. A failing action fails the run with the error, unless the step has `onError: continue`.
- [ ] `flow.<slug>.running` and `flow.<slug>.lastRun.*` update in the state tree and show live in the UI.
- [ ] The UI has a Flows tab that lists, creates and edits a flow (trigger, concurrency, steps with manifest-driven action forms), runs and cancels it, and shows live run status. The journal's `why` drawer shows run and step entries readably.
- [ ] A Rust end-to-end test and a browser smoke test cover the above in CI, with no hardware.

## Phases

### Phase 0: Decisions and docs
- [ ] Write down the M3 subset of the flow spec: the `Flow` fields, the `api` and `event` triggers, the `action` (fixed target only) and `wait` (duration only) steps, `timeout` and `onError`. Everything else is rejected on save with a clear error, not ignored.
- [ ] The REST shape for flows and runs, and the WebSocket additions (if any beyond `flow.*` state), in the [state spec](../specs/state-and-event-log.md).
- [ ] Decide where flow types live. Flows aren't plugin protocol, so serde types in `covalence-core`, with TS types for the UI, are the likely answer. Record it, and say if it needs an ADR.
- [ ] Guard against a flow re-triggering itself (an event trigger on a connection the flow also acts on). Either reject it on save or note it as a known gap.

### Phase 1: Flow model and storage (Rust)
- [ ] `Flow`, `Trigger`, `Step` types with serde, matching the spec's YAML field names.
- [ ] Validation: unique step IDs matching `[a-z][A-Za-z0-9_]*`, a known connection and action, parameters checked against the action's schema, durations that parse.
- [ ] Flows stored in the `entities` table, loaded at startup.

### Phase 2: Flow runtime (Rust)
- [ ] A run is a tokio task with a run ID (ULID), a cancellation token and a position.
- [ ] Step execution: `action` goes through the existing output gate with `cause` → its `run.step` entry; `wait` sleeps on the core clock and wakes on cancel.
- [ ] Per-step `timeout` and `onError`; `steps.<id>` results kept on the run, ready for expressions later.
- [ ] Journal kinds `trigger.matched`, `run.started`, `run.step`, `run.finished`.
- [ ] Concurrency modes per flow. Cancellation is cooperative at step boundaries and during waits. An action already in flight finishes; `action.cancel` is out of scope.
- [ ] Trigger matcher for `event` triggers: a plugin `event` entry from a matching connection and event name starts a run, with `cause` → that entry.
- [ ] `flow.<slug>.running` and `flow.<slug>.lastRun.{status, startedAt, durationMs, error?}` in the state tree.
- [ ] Unit tests on a paused clock: ordering, cancel during wait, every concurrency mode, failure and `onError: continue`.

### Phase 3: HTTP API (Rust)
- [ ] `GET/POST /flows`, `GET/PUT/DELETE /flows/{id}`, `POST /flows/{id}/run` (returns the run ID), `GET /runs` (active and recent), `POST /runs/{run}/cancel`.
- [ ] Validation errors returned per field, so the UI can show them next to the step.
- [ ] End-to-end test for every "Done when" item that the API can reach.

### Phase 4: UI
- [ ] Flows tab: list with enabled toggle, running count and last run status.
- [ ] Flow editor: name, trigger (API or event, with the event picked from the connection's manifest), concurrency, and a reorderable list of steps. Action steps reuse `SchemaForm` for parameters; wait steps take a duration.
- [ ] Run and cancel buttons, live run status from `flow.*` state.
- [ ] Journal summaries and the `why` drawer understand `trigger.matched` and `run.*` entries.

### Phase 5: Acceptance
- [ ] Browser smoke test: create the two-step flow, run it, watch the route change and the run finish, open `why` on the action; cancel a run mid-wait.
- [ ] Both tests run in the existing CI jobs.

## Ordering

Phase 0 comes first. Phases 1 → 2 → 3 run in sequence in Rust. Phase 4 can start once the Phase 0 REST shape is written down, against a stub or the Phase 3 API. Phase 5 joins them.

## Out of scope

QuickJS, expressions, conditions and `{expr}` parameters · `state`, `schedule`, `gesture`, `call`, `osc` and `startup` triggers · blocks (`group`, `branch`, `loop`, `parallel`) and the `waitUntil`, `call`, `set`, `script`, `log` and `stop` steps · dynamic targets and interfaces ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) · `retry` · checkpoints and failover policies · `action.cancel` · the Rule and Flow (node graph) views · YAML export · the project op log and undo · surfaces and gestures.
