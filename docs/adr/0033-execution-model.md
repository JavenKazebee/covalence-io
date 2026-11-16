# 0033 – Execution model: one turn thread writes state; flows, scripts and drawing run beside it

- **Status:** Accepted
- **Date:** 2026-11-16
- **Amends:** [0023](0023-key-rendering-and-binding-evaluation.md) (drawing moves off the binding turn), [0021](0021-expressions-as-sandboxed-snippets.md) (more than one QuickJS runtime)
- **Answers:** [M8 Phase 0](../milestones/m8-dataflow.md) "the graph's home", and [ADR 0029](0029-dataflow-sources-transforms-sinks.md)'s question of how sandboxed reducers fit the 16 ms budget
- **Specs:** [state and event log §1.5](../specs/state-and-event-log.md), [dataflow §5](../specs/dataflow.md), [flows §8](../specs/flows.md)
- **Milestone:** [M6: Primitives](../milestones/m6-primitives.md), Phase 0, since M6 reshapes the evaluator

## Context
- **One QuickJS runtime sits behind a lock today** (`expr::quickjs`), and evaluations run one at a time. Bindings, expressions in action parameters and, later, script steps all share it. A script step with a 10 ms limit can hold up a key redraw.
- **M6 to M9 put more on the hot path:** sources and transforms (M6), cells, reducers and layer resolution (M8), per-instance evaluation and `forEach` (M9). All of it has to settle within one scheduler turn ([dataflow §5.3](../specs/dataflow.md)), and a key has to redraw within 16 ms of a state change.
- **The work has two different shapes:**
  - **Continuous and pure:** apply patches, evaluate cells, bindings and layer fields, resolve layers. It must be fast, ordered and deterministic, since replay depends on it ([0007](0007-event-sourced-core.md)).
  - **Sequential and slow:** flows wait, call devices and run scripts with looser limits.
- **Drawing a key** with `tiny-skia` and `cosmic-text` costs far more than evaluating its binding, and doesn't need to block the next turn.

## Decision

### The turn thread
- **One dedicated OS thread, the turn thread, is the only writer of state.** Each turn it:
  1. takes everything waiting on its input channel (patches from connections, control input, results, timer wakes, project changes)
  2. applies them to the state tree
  3. evaluates cells in dependency order, then layer fields and layer resolution, then display bindings
  4. matches `state` and event triggers, and hands matches to the flow runtime
  5. sends the look changes to the drawing thread, the patches to subscribers and the journal, and sink actions to the output gate
- **It has its own QuickJS runtime,** used only for pure evaluation: expressions in bindings and sources, and transform definitions. Limits are tight (default 2 ms per evaluation). A binding that exceeds them fails as it does now and keeps its previous value.
- **It owns its data without locks.** Other threads read state through snapshots and the change feed, never by locking the tree. Ordering within a turn is fixed, so the same inputs give the same turn, which is what replay needs.

### Beside it
- **The flow runtime stays on tokio.** Runs wait, dispatch actions and receive results as async tasks, and send `set` and `local` writes to the turn thread as inputs.
- **Scripts and one-shot expressions run on a script pool:** a small set of QuickJS runtimes (default: the number of cores, at most 4), each on its own thread, with the flow limits ([0021](0021-expressions-as-sandboxed-snippets.md)). They read a state snapshot taken when the step starts. Cancellation still goes through the interrupt hook.
- **The drawing thread** receives look changes per control, draws them at each output's native size and format, and passes them to the surface's host or the web surface. It drops a frame that's been replaced before it was drawn, so a burst of changes draws the latest look once.
- **I/O stays as tokio tasks:** the plugin host, Orbitals, WebSockets and device I/O feed the turn thread's input channel.

### What runs where

| Work | Where |
|---|---|
| Applying patches, cells, reducers, transforms, bindings, layer resolution, trigger matching, `waitUntil` conditions | Turn thread |
| Flow runs, waits, action dispatch | Tokio |
| Script steps, expressions in action parameters | Script pool |
| Drawing key images | Drawing thread |
| Plugin host, Orbitals, devices, API, WebSockets | Tokio |

## Consequences
- A slow script can't delay a key. The 16 ms budget only has to cover the turn and one draw.
- The turn thread's budget becomes the system's: everything in it is measured, and anything that can be slow (a script reducer, a large `forEach`) is bounded by limits. M8's benchmarks run against the turn, not against individual evaluators.
- Script steps see a snapshot, not live state. A script that needs fresh state reads it in a later step. This matches how runs already see the world between steps.
- `QuickJs` splits into a single-threaded evaluator for the turn and a pool for scripts. `binding::worker` becomes part of the turn thread. The rest of the core keeps its interfaces.
- **Failover keeps one rule:** everything that matters for takeover is a patch on the turn thread's output, so a standby that applies the journal ([0008](0008-redundancy-and-failover.md)) has it. Reducers keep no state outside their cell's value and the state they declare.
- **(Later)** Contexts and compiled bindings can be reused in the turn's runtime, rather than a fresh context per evaluation, once benchmarks show it matters.

## Alternatives considered
- **Keep one runtime behind a lock:** simplest, but any script step can block a redraw, which breaks the budget as soon as M8 adds reducers.
- **Everything on tokio, with the state tree behind a lock:** no new threads, but lock contention on every patch, and no fixed order within a turn for replay.
- **A QuickJS runtime per flow or per binding:** perfect isolation, but memory grows with the project, and a Raspberry Pi with 50 connections can't afford it.
- **Draw on the turn thread, as ADR 0023 does now:** one less thread, but a burst of changes on a 32-key deck spends the turn drawing.
