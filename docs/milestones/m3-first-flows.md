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

## History

The phase-by-phase build log was trimmed once the milestone closed. It's in git: `git log -p -- docs/milestones/m3-first-flows.md`.
