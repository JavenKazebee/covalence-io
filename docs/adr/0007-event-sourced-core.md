# 0007 – Event-sourced core with a single output gate

- **Status:** Accepted
- **Date:** 2026-10-18

## Context
Redundancy, multi-user editing, undo, history, the timeline ("why did this fire?") and replay all need the same thing: a numbered, ordered record of what happened, and one place where outputs can be controlled.

## Decision
- **Two logs** ([spec](../specs/state-and-event-log.md)):
  - **Project op log:** every configuration change. Durable, attributed and replicated.
  - **Runtime journal:** inputs, state patches, run lifecycle, action requests and results, connection status. High volume, with limited retention.
- Every entry has a `seq` (increasing per log), a leadership `term`, a wall-clock `ts`, an `origin`, and a **causation chain** (`cause` and `run`).
- **Every side effect** (action dispatch, surface output, outgoing OSC/HTTP) passes through **one output gate**. The gate checks, in order:
  1. node role (active or standby)
  2. simulate or rehearsal mode
  3. show lock and permissions
  4. term fencing

  It then journals either `action.dispatched` or `action.suppressed`.
- **Non-deterministic inputs** (time, random numbers, action results) are journaled, so a recorded session can be replayed exactly.

## Consequences
- Standby nodes replicate the logs instead of recomputing ([0008](0008-redundancy-and-failover.md)).
- Multi-user editing is ordered operations on the project log ([0009](0009-multi-user-editing.md)).
- The timeline and debugger read the journal directly.
- Every subsystem must go through the gate. No driver or plugin may emit outputs on its own initiative except through actions the core requested.
