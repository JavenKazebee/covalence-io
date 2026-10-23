# 0027 – Events are addressed by state path; control triggers become event triggers

- **Status:** Proposed
- **Date:** 2026-10-22
- **Amends:** [0024](0024-surfaces-as-two-way-devices.md) (`control` triggers and `input.event`), [0019](0019-dynamic-action-targets-and-interfaces.md) (interface events are matched through the same mechanism)
- **Specs:** [dataflow §3](../specs/dataflow.md), [flows §1.1](../specs/flows.md), [surfaces §7](../specs/surfaces.md), [state and event log §2.2](../specs/state-and-event-log.md), [plugin protocol §6](../specs/plugin-protocol.md)
- **Milestone:** [M6: Primitives](../milestones/m6-primitives.md)

## Context
- **Events reach flows in three ways:**
  - `event` triggers for connections
  - `control` triggers for surfaces
  - pattern `state` triggers (planned)

  Each has its own matcher, journal kind and payload shape.
- **ADR 0024 already says a GPIO contact from a plugin and a key on a surface should drive the same flow,** but they're addressed differently: one by connection and event name, the other by layer and control.
- **Other occurrences aren't events at all:** a flow finishing, the core starting, a helper's `select`. Each would need another trigger kind.
- **State is addressed by path,** and every tool (selectors, `why`, the timeline's filters, subscriptions) already understands paths.

## Decision
- **Every event happens at a node in the state tree and has a name.** The text form is `<path>:<name>`:
  - `pp:timerEnded`
  - `vh.outputs["SDI 4"]:routeChanged`
  - `surface.deck.controls.k3:press`
  - `vars.armed:on`
  - `flow.preService:finished`
  - `sys:startup`
- **One journal kind, `event`,** with the path, the name, the payload and its context (`surface`, `layer`, `control`, `item`, `conn`, `key`). `input.event` is folded into it.
- **One trigger kind for events:** `{kind: "event", from: EventSource, event, filter?}`. An `EventSource` is a path selector, a target selector (including interfaces), a layer or surface with a control selector, a group, or a component slot. Every form reduces to a path pattern plus a context filter, and one indexed matcher serves them all.
- **`control` and `startup` triggers are removed.** A `control` trigger becomes `{from: {layer, controls}, event}`; `startup` becomes `{from: {path: "sys"}, event: "startup"}`.
- **One payload shape:** `trigger.path`, `.event`, `.payload`, `.captures`, plus the context fields that apply. A turn's `delta` is `trigger.payload.delta`.
- **Plugins can emit at a node:** `event.emit` gains `at?: Path`, relative to the connection's root.
- **Flows emit `started`, `finished` and `failed`** at `flow.<slug>`, so "when that flow finishes" needs no new trigger kind.

## Consequences
- A flow listening for `press` can name a Stream Deck key, a GPIO pin, an OSC button or a component's slot with the same trigger and the same editor.
- Pattern matching (`surface.*.controls.*:press`) and interface matching come with one matcher instead of three.
- The timeline filters events and state changes the same way.
- Expressions that read `trigger.delta` or `trigger.surface` change shape. M5 projects are migrated by M6, and there are no others.
- `trigger.matched` keeps pointing at the `event` entry, so `why` is unchanged.

## Alternatives considered
- **Keep separate trigger kinds** and add pattern triggers beside them: no migration, but three matchers, three payloads and three editors, and every new event source adds a fourth.
- **Make events state** (a counter per event): one model, but events with payloads don't fit, and "the last press" isn't "a press".
- **Address events by entity ID** rather than path: stable across renames, but entity IDs don't nest, so patterns over collections (`vh.outputs.*`) are impossible. Paths in the journal are stored in ID form anyway.
