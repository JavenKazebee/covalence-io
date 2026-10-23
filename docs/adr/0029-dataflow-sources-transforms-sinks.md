# 0029 – Dataflow is sources, transforms and sinks; helpers are transforms; transforms are sandboxed definitions

- **Status:** Proposed
- **Date:** 2026-10-22
- **Amends:** [0024](0024-surfaces-as-two-way-devices.md) (helpers become reducers; their `to` routes become sinks on any cell), [0025](0025-device-drivers-are-plugins.md) ("never: helpers" becomes "transforms, in the core's sandbox"), [0021](0021-expressions-as-sandboxed-snippets.md) (a new sandboxed context: transform definitions)
- **Answers:** [state spec open question 4](../specs/state-and-event-log.md) for cells and fader input; [bindings open question 2](../specs/bindings.md) (with [0026](0026-sources-and-selectors.md))
- **Specs:** [dataflow §4–§7](../specs/dataflow.md), [bindings §5](../specs/bindings.md), [surfaces §4.3](../specs/surfaces.md)
- **Milestone:** [M8: Dataflow](../milestones/m8-dataflow.md); plugin and project transforms in [M9](../milestones/m9-composition.md)

## Context
- **Several reactive systems are planned, each with its own evaluator, options and validation:**
  - display bindings (state → output)
  - helpers (input → derived state and events, with `to: {action}` routes)
  - layer `when` and `rank`
  - `state` triggers
  - `follow`
- **They are the same idea:** a value computed from state, sometimes with memory, sent somewhere.
- **Helpers only exist on controls.** A GPIO contact from a plugin, an OSC fader or a MIDI note from a connection can't use a `toggle` or a `position`, though ADR 0024 says they should behave like controls.
- **Variables are write-only from flows.** There's no "`vars.isLive` is `atem.program == 3`" without a flow that polls or a `state` trigger that copies.
- **ADR 0025 forbids plugins from defining helpers,** to keep timing, replay and `why` in the core. That's right for code in the plugin host, but it means every new curve or accumulator needs a core release.

## Decision

### Three primitives
- **Sources** ([0026](0026-sources-and-selectors.md)) are where values come from.
- **Transforms** are functions over sources, of two sorts:
  - **stateless:** `format`, `rules`, `map` …
  - **reducers,** which keep a value and step on events or on input changes: `toggle`, `counter`, `position`, `velocity`, `select`, `absolute`, `rate`, `latch`, `script`
- **Sinks** are where values go: a control's outputs (display bindings), a writable variable, or one parameter of an action, rate-limited with the latest value winning.

### Cells
- **A cell is a source with a home in the state tree:**
  - a variable
  - a control binding's helper
  - a composite's state ([0030](0030-composite-connections.md))

  Its value is journaled as a `state.patch` with `cause` → what moved it, so `why` works through it. It's read-only.
- **A reducer needs a home,** so it can only be a cell's source.
- **One graph, one turn.** Cells are evaluated in dependency order within the scheduler turn. Then layer resolution, bindings, state triggers and sinks see the settled state. A cell evaluated twice in a turn is on a cycle, and keeps its previous value with `cycle`.
- **High-rate cells may be journaled sampled:** `journal: {sampled: "100ms"}`.

### Variables are declared entities
- **A variable has a schema, and optionally:**
  - `persist`
  - a `value` source (making it computed or accumulated, and read-only)
  - `to` sinks
- **Undeclared variables still work,** with a warning.

### Helpers are reducers on a control's binding
- **A helper is a reducer whose `on` and `input` default to its control.** Its home is under the layer or the surface, as ADR 0024 put it.
- **A helper's `to` is the general `to` of any cell.** `gestures` timing becomes a setting on the control binding, since it configures events rather than computing a value.
- **Sinks keep things in step; flows do things in order.** A sink never runs steps. "When X, do A then B" stays a flow.

### Transforms are sandboxed definitions
- **Every transform has a definition:** a pure JavaScript module (`defineTransform`) with options, result type and events, plus either `compute`, or `init` and `step`. It runs in the core's QuickJS under the bindings' limits, with time passed in and no host API.
- **The built-ins are written this way first.** The definition is the specification. A native implementation is allowed when a conformance suite shows it gives the same states, values and events.
- **Plugins ship transforms** in their manifest (`<plugin id>/<id>`), and **projects** define their own (`project/<slug>`) (M9). They run in the core, not the plugin host, so timing, replay and `why` are unaffected.
- **Plugins still never define** step kinds, scheduling, gesture timing or layer resolution ([0025](0025-device-drivers-are-plugins.md)).

## Consequences
- Any state can get a toggle, a position or a velocity, not only a control: a GPIO contact can latch, and a MIDI CC can drive a camera's zoom speed.
- Computed variables replace "copy this into a variable" flows, and they're visible in the state tree with `why`.
- One evaluator, one dependency graph and one cycle rule replace four. The binding worker becomes the graph's evaluator for outputs.
- The 16 ms budget now covers cells between a change and a key. Native implementations of the built-ins keep that cheap. Sandboxed reducers cost an evaluation per step, which M8 measures.
- More journal entries: every cell change is a patch. Sampling bounds the high-rate ones.
- The plugin ecosystem can add curves, accelerations and accumulators without a core release, and without touching the guarantees ADR 0025 protected.

## Alternatives considered
- **Helpers only on controls, as ADR 0024 planned:** smaller, but every non-surface input needs a workaround, and value mappings would come back as a second entity.
- **Flows for everything continuous** (`state` trigger → `set`): no new concepts, but runs for every fader movement, no rate limiting, and nothing to show in the state tree as "this value is computed".
- **A general node-graph dataflow with wires:** maximum freedom, but [0018](0018-flows-as-step-trees.md) already chose expressions over wires for being readable, and this keeps that choice: sources name what they read.
- **Plugin-defined helpers in the plugin host:** no new sandbox context, but every input would cross IPC and timing would depend on the host, which ADR 0025 rejected.
