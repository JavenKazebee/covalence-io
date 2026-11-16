# 0030 – Composite connections: users build devices out of other devices

- **Status:** Proposed
- **Revised in place while Proposed:** `delegate`, and standalone impls rejected
- **Amended by:** [0032](0032-one-template-primitive.md) (composite templates are one kind of template; collections through `forEach`; action flows are owned flows)
- **Date:** 2026-10-22
- **Amends:** [0019](0019-dynamic-action-targets-and-interfaces.md) (its rejected "aggregator plugin" alternative returns as a no-code composite)
- **Specs:** [composition §6](../specs/composition.md), [dataflow](../specs/dataflow.md), [flows](../specs/flows.md)
- **Milestone:** [M9: Composition](../milestones/m9-composition.md)

## Context
- **A "camera" on a show is several devices:** PTZ from one connection, tally from the switcher's program and preview, record state from a recorder, maybe an iris from a CCU.
- **Interfaces** ([ADR 0019](0019-dynamic-action-targets-and-interfaces.md)) let one layer control any camera, but only one connection at a time. Tally from the switcher can't be part of "the camera".
- **ADR 0019 rejected aggregator plugins,** because each has to be written by hand, and it hides the real target from the journal.
- **Users already compose these by hand** in Companion with duplicated feedbacks and actions per camera, which is what interfaces were meant to end.

## Decision
- **A composite is a connection** with plugin `core.composite`, defined in the project:
  - **`params`**
  - **`state`:** a tree of sources, each a cell at `<alias>.<path>`
  - **`connected`**
  - **`actions`:** each runs a flow, with its params as the flow's `input` and its `outputs` as the result, or forwards to one action on another target
  - **`events`:** each re-emits events from an event source
  - **`implements`:** interfaces, mapped and checked as a plugin's are
  - **`delegate`:** whole interfaces passed through to another target, `[{interface, to}]`
- **It's a connection in every respect:** an alias, a state root, targets, `{interface}` selection, the connection list. Nothing downstream knows it's composite.
- **Causation runs through it.** A composite action's `action.requested` causes the flow's `run.started`, which causes the real device's `action.requested`. Its cells and events point at what they came from.
- **Composite actions are `internal`** ([0028](0028-the-core-is-a-connection.md)). The actions they cause are gated themselves, so simulate mode still holds.
- **Delegation forwards a whole interface in one line.** `delegate: [{interface: "ptz.speed@1", to: {ref: "params.ptz"}}]` makes every action, event and state member of `ptz.speed@1` on the target the composite's own, and the composite implements that interface. The composite writes only what it adds: "Cam A" delegates `ptz.speed@1` and `ptz.presets@1` to the camera and defines `tally@1` from the switcher.
  - `to` is a `Source<Target>` ([0026](0026-sources-and-selectors.md)), and its constraint has to include the interface, which is checked on save. When it resolves to nothing, the delegated actions fail with `no_target` and the state reads as disconnected.
  - A member that's delegated can't also be defined (`duplicateMember`). There are no overrides here either: to change one member, define the interface's members yourself.
  - A delegated action's `action.requested` causes the target's, with `via` naming the interface, so `why` shows the real camera.
- **Composites nest.** Cycles in state are `cycle`, recursion is stopped by the loop guard, and calls nest at most 16 deep.
- **Flows gain `inputs` and `outputs`** ([flows §1](../specs/flows.md)), which composite actions need and the `call` step already planned.

## Consequences
- "Camera A" is one target: PTZ, tally and record state together, selectable by `{interface: ["ptz.speed@1", "tally@1"]}` like a real camera. Every layer and component written against the interface works with it.
- Users can adapt a device to an interface its plugin doesn't implement, without writing a plugin.
- With `delegate`, wrapping a camera to add tally doesn't mean forwarding every PTZ action by hand, and actions the plugin adds to the interface later appear without editing the composite.
- `why` stays complete, unlike a hand-written aggregator plugin.
- A composite's state is computed, so it's only as fresh as what it reads, and stale when they are.
- The connection form gets a second editor, for composites. Templates ([0031](0031-templates-instances-and-components.md)) make "one composite per camera" a single definition.

## Alternatives considered
- **Standalone interface implementations** (an `impl` entity that maps an existing connection onto an interface, without a new connection): no wrapper, but with capability-sized interfaces and `range` in manifests ([0019](0019-dynamic-action-targets-and-interfaces.md)) an impl would only rename and convert, which is the plugin author's job. A composite with `delegate` covers plugins that don't. Revisit if real projects wrap the same plugin in composites over and over.
- **Aggregator plugins:** possible today, but hand-written per combination, and the journal sees only the aggregator.
- **Interfaces spanning several connections at the target level** (`{conn: [a, b]}`): no new entity, but every place that resolves a target would have to merge state and route actions.
- **Layer-level parameters only** (pass PTZ, switcher and input separately everywhere): works, but every layer, binding and flow repeats the wiring, and nothing can select "a camera" as one thing.
