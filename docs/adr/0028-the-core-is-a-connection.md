# 0028 – The core is a connection; surfaces and flows are its targets

- **Status:** Proposed
- **Date:** 2026-10-22
- **Answers:** [surfaces spec open question 1](../specs/surfaces.md) (core-provided actions)
- **Specs:** [core actions](../specs/core-actions.md), [surfaces §4.5, §9](../specs/surfaces.md), [plugin protocol §4](../specs/plugin-protocol.md)
- **Milestone:** [M6: Primitives](../milestones/m6-primitives.md)

## Context
- **Flows can't change the current layer, inject input or cancel another flow.** M5 left this open: a reserved `core` target, or new step kinds?
- **New step kinds** would each need their own editor, journal entries, gate rules and typing. They wouldn't work with dynamic targets: "navigate whichever deck is selected" needs a target held in state.
- **Actions already have all of that:** typed parameters and results, `action.requested`/`result` in the journal, the output gate, simulate mode, idempotency, and dynamic targets through interfaces ([ADR 0019](0019-dynamic-action-targets-and-interfaces.md)).
- **Sinks and composites** ([ADR 0029](0029-dataflow-sources-transforms-sinks.md), [ADR 0030](0030-composite-connections.md)) drive actions. Core capabilities would be out of their reach as steps.

## Decision
- **A built-in connection, ID and alias `core`,** always present and connected, owns everything Covalence itself can do. Its manifest is listed with the plugins', and its actions run in-process.
- **Its collections are targets.** `surfaces` (keyed by surface ID, interface `surface@1`: `navigate`, `back`, `inject`, `setBrightness`) and `flows` (`flow@1`: `cancelRuns`, `setEnabled`). A variable of kind `target` with interface `surface@1` holds "the selected deck".
- **Its state is the core's roots** (`sys`, `surface`, `layer`, `flow`, `vars`). It has no `core.` subtree, and `core` is reserved as a root so no alias can take it.
- **`ActionDef` gains `effect: "internal" | "device"`.** Simulate mode suppresses `device` effects only, so rehearsing still navigates decks. Role and term apply to everything.
- **No new step kinds.** `call`, `set`, `wait` and the blocks stay steps, because they're about the run itself: they return outputs, write `local` or shape control flow.

## Consequences
- Flows navigate, inject and cancel with the `action` step they already have, typed and journaled like any device.
- "Navigate the selected deck" and "navigate every deck in this list" use dynamic targets and `forEach`, with nothing new.
- Sinks and composites can drive core actions: an encoder `select` reducer with `to: {action: navigate}` pages through layers.
- The engine routes `conn: "core"` to an in-process handler before the host. The `Actions` trait is unchanged.
- `core` joins the reserved names, so a project with a connection aliased `core` is migrated (there are none).

## Alternatives considered
- **New step kinds** (`navigate`, `inject`): obvious in the editor, but each is a one-off with its own typing and gate rules, and none works with dynamic targets.
- **A `core` target without being a connection:** less to explain, but every place that lists connections, checks manifests or resolves interfaces would need a special case.
- **One connection per surface:** a surface as its own target, but surfaces already have a provider connection (M7), and two connections for one device would confuse the UI.
