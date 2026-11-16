# 0034 – Editing a running show: changesets, pinned runs and kept state

- **Status:** Proposed
- **Date:** 2026-11-16
- **Amends:** [0009](0009-multi-user-editing.md) (draft and publish becomes a changeset across entities)
- **Specs:** [flows §8.5](../specs/flows.md), [composition §1](../specs/composition.md), [state and event log §2.3](../specs/state-and-event-log.md)
- **Milestone:** changesets and pinned runs with [M9: Composition](../milestones/m9-composition.md); kept state with [M8: Dataflow](../milestones/m8-dataflow.md)

## Context
- **People edit during services.** A key gets relabelled, a flow fixed, a camera added between songs. [ADR 0009](0009-multi-user-editing.md) allows it, with a show lock and draft and publish for "show-critical pages and flows".
- **Drafts are per entity in 0009,** but with templates and nesting ([0032](0032-one-template-primitive.md)) one change often spans several: a template, its instances' params and a flow it owns. Publishing them one at a time passes through states that were never meant to exist, where an instance's params don't match its template for a moment.
- **Nothing says what running things do when a change lands:**
  - a run in the middle of the flow that changed
  - a helper's value when its layer template changes
  - `forEach` instances when the list's `key` expression changes

  Each could be answered differently per kind, and users would have to learn each answer.

## Decision

### Changesets
- **A draft is a changeset:** a set of operations on any number of entities, made by one user, on top of a base revision of each entity it touches.
- **Publishing applies the whole changeset as one project operation,** in one turn ([0033](0033-execution-model.md)), or none of it. Validation (`badInstance`, `recursiveTemplate`, flow errors) runs against the result, so a changeset that fixes a template and its instances together passes, where either change alone would fail.
- **A changeset whose entities changed underneath it** is rebased field by field as 0009 merges edits. An overlapping field asks the user to choose before publishing.
- **Edits outside a changeset** apply immediately, as now. The show lock forces changesets for everyone below admin.
- **The timeline shows a publish as one entry,** "Ana published 'Camera B fixes' (4 changes)", and undo reverses it as one.

### Runs finish on the version they started with
- **A run pins its flow's revision when it starts.** Steps, inputs and error policies come from that revision until the run ends. Edits apply from the next run.
- **`run.started` records the revision,** so `why` and the timeline show which version did what.
- **Disabling a flow** doesn't stop its active runs, and **deleting one** cancels them, as [flows §8.5](../specs/flows.md) already has it. To stop a run that's using an old version, cancel it (`cancelRuns`, [core actions](../specs/core-actions.md)); the editor offers that when you publish a change to a flow that has active runs.

### State is kept when it still fits
- **A cell keeps its value through an edit** when it's still the same kind of thing: the same reducer (`toggle` is still `toggle`) and a value that still matches its schema. Changed options take effect from the next step: a `position` whose `max` drops is clamped on its next input.
- **Otherwise it resets** to `init`, journaled as a `state.patch` with `cause` → the project operation, so `why` says "reset by Ana's edit".
- **`forEach` instances are matched by key.** If the `key` expression changes, instances whose new key matches an old one keep their state. The rest are added and removed as for a list change, with `cause` → the operation.
- **Identity is the entity, not its position.** Moving a binding to another control, or renaming a slug, keeps the cell. Deleting and recreating it doesn't.

## Consequences
- Fixing a template and its instances is one safe step, not a sequence of broken ones.
- A sequence that's running when someone saves a fix finishes as it began, and the next press runs the fix. Nothing changes partway through a cue.
- A latched key stays latched through a relabel, and a counter keeps counting through a colour change.
- Runs hold an old revision until they end, so a flow's old revisions are kept while any run uses them.
- The undo stack holds changesets as single entries.

## Alternatives considered
- **Per-entity drafts, as 0009 has them:** simpler, but multi-entity changes pass through invalid states, and template edits would need their own atomic mechanism anyway.
- **Runs switch to the new version at the next step:** fixes take effect sooner, but a step ID that no longer exists, or a variable the new version doesn't set, would have to be handled case by case, and a run would mix two versions.
- **Reset all state on every edit:** predictable, but relabelling a key would unlatch it mid-show.
- **Keep state whenever the cell's name matches:** keeps more, but a `toggle` replaced by a `counter` would start from `true`.
