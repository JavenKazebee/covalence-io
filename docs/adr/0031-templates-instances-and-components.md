# 0031 – Reuse is by parameters: templates, instances, components, and explicit scope

- **Status:** Proposed
- **Date:** 2026-10-22
- **Amends:** [bindings §6–§7](../specs/bindings.md) ("Create from template" makes instances, not copies; plugin `presets` become components; the `layer.params` shorthand is replaced by `params`), [surfaces §5.3](../specs/surfaces.md) (the tree preset no longer passes parameters down)
- **Specs:** [composition §2–§3](../specs/composition.md), [dataflow §8](../specs/dataflow.md)
- **Milestone:** [M9: Composition](../milestones/m9-composition.md)

## Context
- **"Create from template" stamps out copies of a layer.** Copies drift: a fix to one camera's layer doesn't reach the others, and nothing records that they were ever the same.
- **What a key does is spread out:** its look and helpers are in a control binding, what happens on press is in one or more flows, and the parameters are on the layer. The editor has to search to answer "what does this key do?".
- **Plugin `presets`** are ready-made bindings, but they can't carry behaviour or parameters.
- **Scope is implicit in places:**
  - the tree preset passes a layer's parameters to its descendants
  - `layer.params.camera` means "this layer's", but `layer` is also the root of every layer's state, so a layer slugged `params` would collide
- **The goal is composition over inheritance:** reuse by putting parts together with parameters, never by copying and editing, and never by overriding what's inside something else.

## Decision

### Templates and instances
- **A layer or a composite with `params` is a template.**
- **An instance is the same entity type** with `template` and `params`, and only its own identity besides. A layer instance also keeps `on`, `when`, `level` and `rank`, which say where it appears, not what it is.
- **No overrides.** An instance can't change its template's bindings, helpers or definitions. "Detach" makes a plain copy, one-way.
- **One level:** a template can't itself be an instance.
- **Editing a template changes every instance.** A change an instance can no longer satisfy is refused, listing the instances.
- **Events from an instance carry it as `layer`,** so one flow on the template handles every instance, with that instance's `params`.

### Components
- **A component is a reusable control:** slots (the controls it needs, with capabilities), each slot's props, helpers and gesture timing, flows that belong to it, and `params`.
- **A placement puts a component on a layer:** slot → control, and its params as sources. It owns those controls.
- **Owned flows run per placement,** with triggers naming slots and the placement's `params` in scope.
- **Plugins ship components** as content with no code, replacing `presets`.
- **Control groups can place a component per item.**

### Explicit scope
- **`params` is always the own container's.** Nothing is inherited implicitly. To pass a value in, declare a parameter and bind it where the thing is placed, in the placer's scope.
- **The absolute path is `layer.<slug>.params.<name>`.** The `layer.params` shorthand is gone.
- **`{scope: "params" | "item" | "self", path}`** refs name scope values in structured sources, as the expression names do.

## Consequences
- A camera layer is defined once and instanced per camera, or once with a selector row ([0019](0019-dynamic-action-targets-and-interfaces.md)). Either way there's one place to fix it.
- "What does this key do?" is "a Tally key, camera = Cam A", with a link to the component.
- Plugin authors can ship behaviour (a "Record" key that toggles and shows the state), not only looks.
- Parameters are written out where they're bound. That's more typing than inheritance, but every value has one visible source, and the editor fills the common case.
- Owned flows need their own state path and journal identity (`<component>/<flow>`), which the timeline and `why` must show.
- The tree preset loses parameter inheritance before it's built. A child layer that needs its parent's camera declares and binds it.

## Alternatives considered
- **Copies** ("Create from template", as now): simplest, but drift is guaranteed, and the user's mental model ("these are all the same") isn't recorded anywhere.
- **Instances with overrides:** what most tools do, but every instance becomes a partial copy, and "where does this value come from?" needs walking a chain. That's inheritance, which this ADR exists to avoid.
- **Lexical inheritance of `params` down the layer tree:** less typing, but values arrive from places the user can't see, and moving a layer changes its meaning.
- **Components as plain flows plus bindings with a naming convention:** no new entity, but nothing ties them together, so neither the editor nor a plugin can treat them as one thing.
