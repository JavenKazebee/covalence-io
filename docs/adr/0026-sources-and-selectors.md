# 0026 – One way to give a value, one way to pick things: sources and selectors

- **Status:** Proposed
- **Date:** 2026-10-22
- **Amends:** [0023](0023-key-rendering-and-binding-evaluation.md) (the five binding kinds become sources and stateless transforms; native evaluation stands), [0019](0019-dynamic-action-targets-and-interfaces.md) (`TargetSpec` becomes a `Source<Target>` with a constraint)
- **Specs:** [dataflow §1, §2, §4.1](../specs/dataflow.md), [bindings](../specs/bindings.md), [surfaces](../specs/surfaces.md), [flows](../specs/flows.md)
- **Milestone:** [M6: Primitives](../milestones/m6-primitives.md)

## Context
- **"A literal, a state path or an expression" is defined over and over,** each time a little differently:
  - binding kinds (`value`, `template`, `rules`, `map`, `expr`)
  - helpers' `Opt<T>`
  - `{expr}` action parameters
  - `wait.for`
  - a layer's `rank` and `when`
  - `TargetSpec`
- **"Which things" is defined at least four times:**
  - `ControlRef`, with seven variants
  - a layer's `controls`, with three forms
  - a control group's `controls`
  - `TargetSpec` and `TargetConstraint`, and a layer's `on`
- **Most of these are still marked Later.** Every new form (region, neighbour, capability) would have to be added to each of them, and each copy would drift. Users would meet a different editor widget for the same idea in each place.
- **Binding conditions can't be combined** (bindings open question 2), because a condition isn't itself a value.

## Decision
- **`Source<T>` is the one shape for a value that may follow state:** a literal, `{value}` (an escaped literal), `{ref}`, `{expr}`, or `{use}` (a transform). It has one serde type, one evaluator with run-time read tracking, one typing rule and one editor widget. Every field that can follow state is a `Source`.
- **Sources are recognised by shape, only where the schema says so.** Nested values are literals, as action parameters already work.
- **The binding kinds become stateless transforms:** `format`, `template`, `rules` and `map`, joined by `compare`, `all`, `any` and `not`. Their options are sources, so they chain, and conditions combine. ADR 0023's rule stands: they're evaluated natively and tested against their compiled expressions.
- **`Selector<Id>` is the one shape for picking things:** an ID, a list, `{expr}`, the combinators `any`, `all` and `not`, and forms per domain:
  - controls: `region`, `near`, `has` (by capability)
  - targets: `plugin`, `interface`
  - paths: `match`
  - entities: tags (Later)

  It's resolved by one module, reactively where its position is reactive.
- **A dynamic target is `target: Source<Target>` plus `constraint`.** A fixed target is a literal.
- **M5's stored shapes change now** (bindings, `control` triggers, action parameters), before hardware surfaces start creating entities in real projects. Fixtures are migrated, not kept beside.

## Consequences
- Adding a selector form or a transform makes it available everywhere at once, in the core and the editor.
- Bindings get AND, OR and NOT without a new kind.
- Stored JSON gets one level deeper in places (`{use: "format", input: {ref}}` instead of `{kind: "value", path}`). The editor hides that, and the YAML export stays readable.
- The binding editor, the trigger editor and the step editor share their value and selector widgets.
- M5's binding code is reshaped, not rewritten: the native evaluator and `to_expr` stay, keyed by `use`.

## Alternatives considered
- **Keep each shape local,** and add forms where needed: no migration, but every form is built and documented several times, and the copies drift.
- **Expressions only,** with no structured forms: one shape, but users would have to write code for everything, and the editor couldn't offer forms or type-check literals. ADR 0023's native fast path would go too.
- **A separate `kind` tag on every source** (`{kind: "literal", value: 3}`): no shape rules, but every literal in every project gets wrapped, and hand-written YAML gets noisy.
