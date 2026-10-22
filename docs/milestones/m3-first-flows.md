# Milestone 3: First flows

- **Status:** Done
- **Started:** 2026-10-21

The first automation. A flow is a trigger plus a flat list of `action` and `wait` steps, which is the Button-view shape from the [flow spec](../specs/flows.md) §6. A flow can be started from the API or the UI, or by a plugin event. Every run is journaled step by step, so `why` walks from a device action back through the run to whatever started it. QuickJS stays out: parameters are literal values and triggers have no filters or conditions.

## Done when

- [x] A flow can be created, listed, updated and deleted over `POST/GET/PUT/DELETE /flows`, stored as a `flow` entity, and it survives a server restart.
- [x] `POST /flows/{id}/run` on a flow with steps `route (vh 1←3)`, `wait 1s`, `route (vh 1←4)` journals `run.started` → `run.step` started/done for each step → `run.finished ok`. `vh.outputs[1].source` reads 3, then 4.
- [x] A flow with an `event` trigger on `vh` `routeChanged` that routes `vh2` runs whenever `vh` changes. `GET /journal/{seq}/why` on its `vh2` `action.result` walks back through `run.step` → `run.started` → `trigger.matched` → `event` → `vh`'s `action.requested`.
- [x] All four concurrency modes behave as the spec says: `parallel` starts a second run, `queue` runs it afterwards, `restart` cancels the active run first, and `ignore` journals `trigger.matched` with `blocked: true`.
- [x] `POST /runs/{run}/cancel` stops a run during a wait, which journals `run.finished cancelled`. A failing action fails the run with the error, unless the step has `onError: continue`.
- [x] `flow.<slug>.running` and `flow.<slug>.lastRun.*` update in the state tree and show live in the UI.
- [x] The UI has a Flows tab that lists, creates and edits a flow (trigger, concurrency, steps with manifest-driven action forms), runs and cancels it, and shows live run status. The journal's `why` drawer shows run and step entries readably.
- [x] A Rust end-to-end test covers the above in CI, with no hardware (`crates/covalence-server/tests/flows_api.rs`). The browser smoke test was dropped from M3 (Phase 5).

## Phases

### Phase 0: Decisions and docs
- [x] The M3 subset of the flow spec: `api` and `event` triggers, fixed-target `action` and duration `wait` steps, `timeout` and `onError`, validation, run semantics and causes ([flow spec](../specs/flows.md) §8). Everything else is rejected on save.
- [x] The REST shape for flows and runs ([flow spec](../specs/flows.md) §9). No new WebSocket messages: status is `flow.*` state, runs are journal entries.
- [x] Flow types are serde types in `covalence_core::flow`, mirrored by hand in the UI, with shared JSON fixtures to catch drift ([ADR 0020](../adr/0020-project-entity-types-in-the-core.md)).
- [x] Self-triggering: a run-time loop guard blocks a trigger that descends from a run of the same flow, plus a save-time warning ([flow spec](../specs/flows.md) §8.6). Live device echoes without a `cause` stay a known gap.

### Phase 1: Flow model and storage (Rust)
- [x] `Flow`, `Trigger`, `Step` types with serde in `covalence_core::flow`, matching the spec's field names. `Flow::parse` reports unsupported spec features with their path.
- [x] Validation per flow spec §8.4, returning `{path, code, message}` errors and warnings. Action params are checked by `covalence_core::schema`, a port of the SDK's validator.
- [x] Fixtures in `crates/covalence-core/fixtures/flows/`, valid ones and `problems/` with their expected errors and warnings.
- [x] `Origin::Flow` gets an optional `step`.
- [x] Flows stored in the `entities` table, loaded at startup (`App::put_flow`, `delete_flow`, `flows`).

### Phase 2: Flow runtime (Rust)
- [x] A run is a tokio task with a run ID (ULID), a cancellation token and a position (`covalence_core::flow::runtime`).
- [x] Step execution: `action` goes through the existing output gate (via the new `Actions` trait) with `cause` → its `run.step` entry; `wait` sleeps on the tokio clock and wakes on cancel.
- [x] Per-step `timeout` and `onError`. Keeping `steps.<id>` results moves to the expressions milestone, the first thing that reads them.
- [x] Journal kinds `trigger.matched`, `run.started`, `run.step`, `run.finished`.
- [x] Concurrency modes per flow. Cancellation is cooperative at step boundaries and during waits. An action already in flight finishes; `action.cancel` is out of scope.
- [x] Trigger matcher for `event` triggers: a plugin `event` entry from a matching connection and event name starts a run, with `cause` → that entry.
- [x] Loop guard (flow spec §8.6) and the queue limit of 16.
- [x] `flow.<slug>.running` and `flow.<slug>.lastRun.{status, startedAt, durationMs, error?}` in the state tree, stored under the root key `flow` where a connection uses its ID. The server doesn't show the `flow` root over HTTP or the WebSocket yet (Phase 3).
- [x] Tests on a paused clock: ordering, cancel during wait and during an action, every concurrency mode, failure and `onError: continue`, timeouts, the loop guard, event triggers and `why` (`crates/covalence-core/tests/flow_runtime.rs`).
- [x] The server starts the runtime with the stored flows and keeps it in step with `put_flow` and `delete_flow`.

### Phase 3: HTTP API (Rust)
- [x] The endpoints in [flow spec](../specs/flows.md) §9 (`crates/covalence-server/src/flows.rs`), added to the API table in `covalence-server`'s `lib.rs`.
- [x] `GET /state` and the WebSocket serve the `flow` root.
- [x] Validation errors returned per field, so the UI can show them next to the step.
- [x] End-to-end test for every "Done when" item that the API can reach (`crates/covalence-server/tests/flows_api.rs`). The `Server` helper moved to `tests/common/`.

### Phase 4: UI
- [x] Flows tab: list with enabled toggle, running count and last run status (`src/views/FlowsView.vue`).
- [x] Flow editor: name, trigger (API or event, with the event picked from the connection's manifest), concurrency, and a reorderable list of steps. Action steps reuse `SchemaForm` for parameters; wait steps take a duration. Server problems show next to the field their path names (`src/components/FlowEditor.vue`, `StepEditor.vue`).
- [x] Run and cancel buttons, live run status from `flow.*` state. The runs table refetches `GET /runs` on the flow's journal entries, so the current step is highlighted as it moves (`src/components/FlowRuns.vue`).
- [x] Journal summaries and the `why` drawer understand `trigger.matched` and `run.*` entries, name flows, and link to them.
- [x] Flow types in `src/flows/model.ts`, tested against the shared fixtures in `crates/covalence-core/fixtures/flows/`: every valid fixture round-trips through the editor unchanged.

### Phase 5: Acceptance
- [x] The Rust end-to-end test runs in the existing `cargo test` CI job.
- Dropped: a browser smoke test for flows. The Flows UI was checked by hand against a real server (create the two-step flow, run it, watch the route change and the run finish, open `why` on the action, cancel a run mid-wait), and its flow types are tested against the shared fixtures. Nothing in CI checks the UI and the flows API together, so a break there will only show in use. The M2 smoke test (`packages/ui/e2e/`) is where one would go.

## Ordering

Phase 0 comes first. Phases 1 → 2 → 3 run in sequence in Rust. Phase 4 can start now that the REST shape is written down, against a stub or the Phase 3 API. Phase 5 joins them.

## Out of scope

QuickJS, expressions, conditions and `{expr}` parameters · `state`, `schedule`, `gesture`, `call`, `osc` and `startup` triggers · blocks (`group`, `branch`, `loop`, `parallel`) and the `waitUntil`, `call`, `set`, `script`, `log` and `stop` steps · dynamic targets and interfaces ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) · `retry` · checkpoints and failover policies · `action.cancel` · the Rule and Flow (node graph) views · YAML export · the project op log and undo · surfaces and gestures.
