# 0018 – Flows are structured step trees; bindings are expressions

- **Status:** Accepted
- **Date:** 2026-10-18
- **Specs:** [flows](../specs/flows.md), [bindings](../specs/bindings.md)

## Context
- A flow is the single unit of automation, edited in three views: Button, Rule and Flow (node graph) ([architecture](../architecture.md#three-ways-to-edit-one-model)). The docs didn't say how a flow is stored, or whether the node graph is control flow or dataflow.
- Two storage models were on the table:
  - **Free graph:** nodes plus edges, any node can connect to any other.
  - **Structured step tree:** sequence, branch, loop and parallel are blocks that contain their steps.
- For control flow the two are equally expressive. Any goto-style graph can be rewritten with sequence, selection, iteration and a variable. The difference is in ergonomics and in what the runtime has to support.
- Requirements already decided elsewhere depend on the choice:
  - moving a flow between views, including back down to Button view ([architecture](../architecture.md#three-ways-to-edit-one-model))
  - resuming a run from a checkpoint after failover ([ADR 0008](0008-redundancy-and-failover.md))
  - cooperative cancellation at every step boundary
  - deterministic, reviewable YAML export ([ADR 0006](0006-project-storage-and-versioning.md))
  - journaling every step with causation ([ADR 0007](0007-event-sourced-core.md))
- Tools that use free graphs are mostly **dataflow** tools (TouchDesigner, Max, shader editors, Node-RED), where edges carry values. Covalence flows are step sequences, which is the cue-list model of QLab, Companion action lists and Home Assistant scripts.
- Covalence does have dataflow needs (a label showing a timer, a fader driving a volume), but those belong to bindings, not flows.

## Decision
- **Flows are control flow only, stored as a structured step tree.**
  - Blocks: sequence, branch, loop, parallel, group (with catch and finally). Leaf steps: action, wait, wait-until, call flow, set, script, log, stop, break, continue. See the [flow spec](../specs/flows.md).
  - Every step has a stable ID that is unique within its flow.
  - No goto. Retrying, early exit and loop control are steps or step policies instead.
- **The node-graph view draws the tree.** Blocks appear as nested regions, edges mean "then", and there are no data wires. Node positions are editor-only metadata, stored beside the tree and ignored by the runtime.
- **Data inside a run moves through expressions,** not edges. A step reads `trigger`, `input`, `local`, `steps.<id>` and the state tree.
- **Types are checked when editing.** The editor generates TypeScript definitions for each flow's scopes. Values stay dynamic at runtime ([ADR 0004](0004-javascript-expressions-and-scripts.md)).
- **Bindings are dataflow, written as expressions.** A binding is stored in a structured form (`value`, `template`, `rules`, `map` or `expr`). Every form except `expr` compiles to an expression, so the same QuickJS engine evaluates all of them. Converting a binding to `expr` is one-way. See the [binding spec](../specs/bindings.md).

## Consequences
- A flow whose body is a flat sequence *is* a Button-view flow, so moving down a view is a shape check, not a graph analysis.
- A run's position is a path of step IDs. A checkpoint is that position plus the run's locals, which keeps `resume` on failover simple.
- Parallel blocks own their children, so "wait for all" and "first one wins" are well defined. The free-graph fork/join bug, where a join waits for a path that went elsewhere, can't happen.
- Cancellation and error handling follow the nesting: cancelling a block cancels its children, and a group's `catch` covers exactly the steps inside it.
- Exported YAML nests like code and diffs cleanly.
- State machines and jump-back logic are less direct. They need a loop, a variable, separate flows or a script step. If real projects need them often, a dedicated state-machine flow type can be added later as its own construct.
- The Flow view needs a custom nested-block editor with auto-layout, not an off-the-shelf free node canvas. Vue Flow ([ADR 0005](0005-web-ui-vue-tauri-shell.md)) can still render it, with nested regions as group nodes, but layout and drop zones are our code.
- Visual wiring for bindings, if ever wanted, would be a separate editor for bindings and wouldn't change how flows are stored.

## Alternatives considered
- **Free graph of nodes and edges:** the most familiar canvas and the cheapest editor to build. It makes view round-tripping, checkpoint and resume, parallel joins and YAML diffs harder, and adds validation for unreachable nodes, dangling edges and accidental cycles.
- **Free graph limited to structured shapes** (every fork has a matching join): free wiring on screen, but users hit "this connection isn't allowed" errors that are hard to explain, and the runtime still needs a tree internally, so we'd maintain both.
- **Mixed control and data edges in one canvas,** as in Unreal Blueprints: powerful, but known for unreadable graphs, and it duplicates what expressions already do.
- **Bindings stored only as expressions, with the UI recognising patterns:** one model, but pattern recognition over arbitrary JS breaks after a single hand edit.
