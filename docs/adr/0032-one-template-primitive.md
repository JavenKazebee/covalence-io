# 0032 – One reuse primitive: templates and instances, for layers, composites, components and flows

- **Status:** Proposed
- **Date:** 2026-10-23
- **Amends:** [0030](0030-composite-connections.md) (composites get collections through `forEach`, and run owned flows), [0031](0031-templates-instances-and-components.md) (templates nest; components and placements become a template kind and its instances; "one level" now bans only deriving one template from another; the flow on a template becomes an owned flow), [0019](0019-dynamic-action-targets-and-interfaces.md) (a composite's per-item instances are targets through `at`)
- **Specs:** [composition](../specs/composition.md), [dataflow §8](../specs/dataflow.md), [flows §1, §3](../specs/flows.md), [surfaces §1, §6](../specs/surfaces.md), [state and event log §2](../specs/state-and-event-log.md)
- **Milestone:** [M9: Composition](../milestones/m9-composition.md)

## Context
- **M9 plans four ways to reuse something with parameters:**
  - layer templates and their instances
  - components and their placements
  - composite templates and their instances
  - flows with `inputs`, called by `call` steps and composite actions

  They work the same way: declare parameters, bind them where the thing is used, evaluate the body with those values. But each would build its own parameter binding, scope rule, per-instance evaluation, journal identity (`layer` carrying the instance, `placement`, `<component>/<flow>`), `badInstance` check and "Detach".
- **That's the problem [ADR 0026](0026-sources-and-selectors.md) fixed for values and selections.** The copies would drift, and users would learn four editors for one idea.
- **Reuse stops at one level.** Templates can't nest, and components can't contain components (composition open question 1). A camera strip made of tally keys has to be one flat component.
- **Lists that change while the show runs are handled in one place only:** a control group can place a component per item. A composite per camera in a list, a layer per deck, or a watchdog flow per recorder needs a copy per item, or waits for "(Later) `items`".
- **Some scope is still implicit.** A flow triggered by a control event gets the controlling layer's `params`, though nothing passed them to it. That contradicts ADR 0031's own rule that `params` is always the own container's.

## Decision

### One primitive
- **A template is any entity that declares `params`.** Four kinds of entity can: layers, composite connections, components and flows. Every `params` is a map of `ParamDef`s, with the same schema, defaults, editor form and type check.
- **An instance is an entity with `template` and `params`,** plus its own identity, and an optional `forEach`. Bindings are sources ([ADR 0026](0026-sources-and-selectors.md)), evaluated reactively in the placer's scope. One module in the core binds, validates and evaluates instances of every kind, and the UI has one "instance of X, with these params" panel.
- **A component placement is an instance** of a component template. A composite instance is a connection, a layer instance is a layer, and a flow instance is a flow. The kinds differ only in their bodies, and in the fields that say where an instance appears (a layer's `on`, `when`, `level` and `rank`, or a placement's controls).
- **A template is usable itself where its kind can be:** a layer is shown, a composite connects, a flow runs. Missing params take their defaults. A template with a required param that has no default is **abstract**. It can only be used through instances, and a component always is one.
- **No overrides, and no derivation.** An instance can't change, add to or remove anything in its template's body. A template can't be an instance of another template. Detach is still the way out.

### Bodies, and owned flows
- **A template's body** is the entity, plus everything that belongs to it:
  - a layer's control bindings, control groups, placements and owned flows
  - a composite's state, actions, events and owned flows
  - a component's slots, bindings and owned flows
  - a flow's steps
- **Owned flows run per instance,** with that instance's `params`. Flows gain `owner` (a layer or a composite), and components embed theirs, so a plugin can ship them. One flow handles every camera layer because it's owned by the layer template, not because a free-standing flow inherits a layer's params.
- **Free-standing flows only see their own `params`.** A flow triggered by a control event still gets `trigger.layer` and `trigger.instance`, but no longer the layer's `params`.

### Params and inputs
- **Flows keep `inputs` beside `params`,** with the same `ParamDef` shape, the same binding shape and the same editor:
  - **`params`** are bound where an instance is placed. They follow state for as long as the instance exists, and they give it an identity and a home.
  - **`inputs`** are bound each time something is invoked: a `call` step, a composite action, the API, OSC. They're evaluated once when the run starts, and they're in scope as `input`.
- **A run isn't an instance.** Runs already have a handle, a status and a journal identity, so a call doesn't create a new state home.

### Nesting
- **A template's body can contain instances of other templates:**
  - components in a component (`parts`, with the inner slots mapped to the outer component's slots)
  - placements on a layer template
  - flow instances owned by a layer, a composite or a component
- **Each nested instance binds its params in its container's scope,** so `params` is always the innermost instance's own. A value passes down one visible binding at a time, the way arguments pass into a function call.
- **Containment can't be recursive.** A template that contains itself, directly or through others, is a save error (`recursiveTemplate`). Nesting is at most 16 deep (`tooDeep`), the same limit composites use for calls.
- **(Later)** A composite contains composite instances. Composites already combine others by reference, through target params.

### One instance per item
- **`forEach` makes one instance per item** of a live list (a `Source<Value[]>`) or of a target selector (`{targets: Selector<Target>}`). `item`, `index` and `key` are in scope while that instance's params are bound, and only there: inside the body, the item is whatever param it was bound to.
- **Per-item instances live at `<home>.items[<key>]`,** keyed by `key` (an expression; the default is the item itself, or a target's ID form). A reorder doesn't reset them.
- **They come and go with the list.** An added item creates an instance, with its cells starting from `init`. A removed item ends one, and its flows' runs are cancelled with `removed`. Both are journaled as `instance.added` and `instance.removed`, with `cause` → the patch that changed the list.
- **A composite with `forEach` is a connection with a collection.** Each item is a target, `{conn, key: ["items", key]}`, and implements the composite's interfaces through `at: ["items"]` ([ADR 0019](0019-dynamic-action-targets-and-interfaces.md)). "A camera per camera in this list" is one entity, selectable by `camera.ptz@1`.
- **A control group is the `forEach` of placements,** because it also decides which control each item takes, and scrolls. Its `component` and `params` become `use: {template, params}`.
- **A `forEach` has at most 256 items.** Extra items aren't instanced, and are reported as `tooMany`.

### Identity, validation and Detach
- **Every journal entry made inside an instance carries `instance`.** It's the chain of instances from the outermost one: `[{template, instance, key?}]`. This replaces the separate `layer`-as-instance, `placement` and `<component>/<flow>` identities, and `why` shows the chain ("Tally key, camera = Cam B, in Cameras[camB]").
- **One validation, `badInstance`:** a missing required param, a binding of the wrong type, a slot that doesn't fit. A change to a template that an instance can no longer satisfy is refused, listing the instances.
- **One Detach, for every kind.** An instance becomes a plain entity with its body copied and its params left as they were bound. A `forEach` instance detaches to one plain entity per current item, with the item's values written in as literals.

## Consequences
- Binding, scope, evaluation, identity, validation and Detach are built once in the core and shown once in the UI. A new template kind, such as a surface or a variable, only needs its body defined.
- A camera strip made of tally keys, a layer template that places components, and a composite per camera in a live list all work, with no copies.
- "Where does this value come from?" is still answered in one hop per level: a param's binding is beside the instance that uses it. Nesting adds levels, not chains, and the instance chain in the journal shows them.
- Flows triggered from layer templates have to be owned by the template. M5 and M6 flows that read the layer's `params` through a control trigger don't exist yet, so nothing is migrated.
- Per-item instances are created at run time, which ADR 0031 avoided ("nothing is generated"). They're never saved: the entity is the one `forEach` instance, and its items come from state. Their cells cost the same as hand-made instances, bounded by `tooMany` and the 16 ms budget.
- `navigate` gains `item` for a layer instanced per item ([core actions §3.1](../specs/core-actions.md)).
- Transform options aren't params. Transforms are functions with no identity, and reducers already have their homes in cells. Their option schemas use `ParamDef`, so they share the editor form.

## Alternatives considered
- **Keep four mechanisms, as ADR 0030 and 0031 have them:** each simpler alone, but four implementations of the same rules, and every later improvement (nesting, per-item instances, identity in `why`) built four times.
- **A single `template` entity, with the body as a kind-specific payload:** one entity type, but layers, composites, components and flows each have their own store, API and editor, and an instance has to be its kind's entity anyway (a connection, a layer) to appear where that kind does.
- **Merge flow `inputs` into `params`:** one name, but every call would create an instance, with a state home and a lifetime, for something that already has a run.
- **Lexical scope for nested templates** (the inner one sees the outer one's `params`): less typing, but values arrive from places the user can't see, which ADR 0031 rejected.
- **Templates derived from templates** (an instance that is also a template with more params): reuse by partial application, but each level becomes a partial override, and "where does this value come from?" turns into walking a chain again.
- **Per-item copies made by the editor** ("Create from template" per target): no run-time instances, but copies don't follow the list while the show runs.
