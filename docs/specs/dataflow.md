# Spec: Dataflow

- **Status:** Draft v0.1
- **Related:** [ADR 0026](../adr/0026-sources-and-selectors.md), [ADR 0027](../adr/0027-events-addressed-by-path.md), [ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md), [ADR 0031](../adr/0031-templates-instances-and-components.md), [ADR 0032](../adr/0032-one-template-primitive.md), [bindings](bindings.md), [surfaces](surfaces.md), [flows](flows.md), [state and event log](state-and-event-log.md), [composition](composition.md)

This spec defines the small set of primitives that everything reactive in Covalence is built from. Bindings, helpers, layer fields, state triggers, variables and composite connections are all combinations of them.

| Primitive | Is | Section |
|---|---|---|
| **Source** | Where a value comes from: a literal, a state path, an expression or a transform | §1 |
| **Selector** | Which things: controls, targets, paths or entities | §2 |
| **Event address** | Where an event happens: a node in the state tree, and a name | §3 |
| **Transform** | A function over sources: stateless (computed) or stateful (a reducer) | §4 |
| **Cell** | A source with a home in the state tree, so others can read it | §5 |
| **Sink** | Where a value goes: a control output, a variable or an action parameter | §7 |
| **Scope** | The names an expression can see where it's evaluated | §8 |

**Flows do things in order; dataflow keeps things in step.** Anything with order, waits, conditions or retries is a flow ([flows spec](flows.md)). Anything that should continuously follow state is dataflow. A sink never runs steps, and a flow never keeps something in step by polling.

Shapes use TypeScript notation. The source of truth is the Rust types in `covalence-core` ([ADR 0020](../adr/0020-project-entity-types-in-the-core.md)).

**Markers.** **(M6)**, **(M8)** and **(M9)** name the milestone that implements a rule: [M6](../milestones/m6-primitives.md), [M8](../milestones/m8-dataflow.md), [M9](../milestones/m9-composition.md). **(Later)** has no milestone yet.

---

## 1. Sources (M6)

A **source** is where a value comes from. Every field that can be either a literal or follow state is a `Source<T>`, with one serde shape, one evaluator and one editor widget.

```ts
type Source<T> =
  | T                                         // a literal
  | { value: T }                              // a literal that would otherwise read as a source
  | { ref: Ref; fallback?: T }                // one state path
  | { expr: Expr; fallback?: T }              // a sandboxed expression (ADR 0021)
  | { use: string; fallback?: T; [option: string]: unknown };   // a transform (§4)

type Ref =
  | { conn: string; path: Path }              // a connection's state, ID form (state spec §1.1)
  | { target: Source<Target>; path: Path }    // through a target (ADR 0019)
  | { root: "vars" | "sys" | "surface" | "layer" | "flow"; path: Path }
  | { scope: string; path: Path };            // (M9) a scope name (§8): params, item, self

type Expr = string;
type Path = PathSegment[];                    // ADR 0017
```

### 1.1 Recognising a source
- **A source is recognised by its shape,** only where the schema says a field is a `Source`: an object whose keys are exactly `ref`, `expr` or `value`, or `use` with that transform's options, plus an optional `fallback`. Any other value is a literal.
- **Nested values are literals.** Inside a literal object or array, `{expr: …}` is just data, as action parameters already work ([flows §8.3](flows.md)).
- **`{value}` escapes a literal** that would look like a source, in a field whose `T` is an object or any value. The editor adds and removes it.
- **Fields that are always expressions** (`when`, `until`, `filter`) keep plain strings as expressions ([flows §7](flows.md)). An object there is a source, so a `rules` or `compare` transform can drive a state trigger.

### 1.2 Evaluation
- **One evaluator for every source.** Reactive positions (display bindings, layer fields, state triggers, cells) re-evaluate when their reads change (§5.3). One-shot positions (an action step's parameters, `wait.for`) evaluate once, when the step starts. The result is the same either way.
- **Reads are tracked at run time** ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)), through `ref`, `expr`, `target()` and every transform's own sources.
- **`undefined` or a failure gives `fallback`,** then the position's default. Failures are reported with the position's path (`["props", "label"]`) and rate-limited, as for bindings now.
- **Literals cost nothing:** a literal is never evaluated, read-tracked or journaled.
- **Typing.** The editor checks each source against `T`: a literal directly, a `ref` by the state schema, a transform by its declared result type, an `expr` with the generated TypeScript definitions ([flows §4](flows.md)).

### 1.3 Where sources replace older shapes

| Was | Now |
|---|---|
| A binding's `kind: "value" \| "template" \| "rules" \| "map" \| "expr"` ([bindings §2](bindings.md)) | `{ref}`, `{expr}`, or `{use: "format" \| "template" \| "rules" \| "map"}` |
| A binding condition `{path, op, value}` | `{use: "compare", input, op, value}`, combined with `all`, `any`, `not` |
| `Opt<T>` in helper options | `Source<T>` |
| `{expr}` action parameters | `Source<T>` per parameter |
| `wait.for: Duration \| {expr}` | `Source<Duration>` |
| `TargetSpec` `{conn, key?} \| {expr, constraint}` | `target: Source<Target>`, and `constraint` beside it when the target isn't a literal |
| A layer's `rank: number \| {expr}`, `when: {while} \| {expr}` | `Source<number>`, `Source<boolean>` (surfaces §5) |

---

## 2. Selectors (M6)

A **selector** picks a set of things. Every field that names one or many controls, targets, paths or entities is a `Selector`, resolved by one module (`covalence_core::select`).

```ts
type Selector<Id> =
  | Id                                        // one
  | Id[]                                      // these
  | { expr: Expr }                            // computed: an ID, a list of IDs, or targets
  | { any: Selector<Id>[] }                   // union
  | { all: Selector<Id>[] }                   // intersection
  | { not: Selector<Id> }                     // everything in the domain except these
  | DomainForms<Id>;
```

| Domain | `Id` | Extra forms | Universe for `not` |
|---|---|---|---|
| **Controls** | a control ID | `{region: Rect}`, `{near: "left" \| "right" \| "above" \| "below", of: string}`, `{has: {inputs?, outputs?}}` | the controls of the layer, surface or group it's on |
| **Targets** | `Target` (`{conn, key?}`; a string is a connection ID) | `{plugin: string}`, `{interface: string}` | every connection and item |
| **Paths** | `Ref` | `{match: PathPattern}` | none: `not` only inside `all` |
| **Entities** | an entity ID | `{tag: string}` (Later) | every entity of the type |

- **Controls are resolved per surface.** A layer's `controls: {region}` covers whatever controls lie in the rectangle on each surface (surfaces §2). `has` filters by capability: `{has: {inputs: ["relative"]}}` is every encoder.
- **`{interface}` resolves to every target implementing it,** including items through `at` ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) and composites that declare it, or each of their items (composition §6).
- **`PathPattern`** is a text path where `*` matches one segment: `surface.*.controls.*.pressed`. The segments it matched are its **captures**, in order.
- **Reactive selectors re-resolve** when their reads change, or when the domain changes: a control attaches, a connection is created, an interface gets a new implementer.
- **Order is stable:** a selector's result is in the domain's order (controls by `at`, row-major; targets and entities by ID), so a control group's items line up the same way every time.
- **Validation:** an ID that doesn't exist is an error (`unknownControl`, `unknownConnection`, `unknownLayer` …), except inside `expr`, where it resolves to nothing at run time.

---

## 3. Event addresses (M6)

Every event happens **at a node in the state tree** and has a **name**. The text form is `<path>:<name>`.

| Events | Address | Examples |
|---|---|---|
| A connection's | its root, or a node under it (`at`, plugin protocol §6) | `pp:timerEnded`, `vh.outputs["SDI 4"]:routeChanged` |
| A control's input | the control | `surface.deck.controls.k3:press`, `surface.mixer.controls.dial1:turn` |
| A transform's (M8) | its cell (§5) | `vars.armed:on`, `layer.cams.controls.dial1.camera:select` |
| A flow's | the flow | `flow.preService:started`, `flow.preService:finished`, `flow.preService:failed` |
| A composite's (M9) | the composite's root | `camA:tallyChanged` |
| The core's | `sys` | `sys:startup` with `{reason: boot \| failover \| publish}` |

- **Events are journaled as one kind,** `event` ([state spec §2.2](state-and-event-log.md)), with the node's path in ID form, the name, the payload and its context: `surface`, `layer`, `control` and `item` for control input; `conn` and `key` for a target's. `input.event` is folded into it.
- **The payload is the event's own data:** `{delta}` for `turn` and `hold`, `{value}` for `change`, the `EventDef` payload for a plugin's.
- **Interface events** are reached through the interface's name. A plugin's `tallyChanged` mapped to `camera.ptz@1`'s `tally` is addressable as either.

### 3.1 Event sources

An event trigger (flows §1.1), a reducer's `on` (§4.2) and a composite's event (composition §6) all name events the same way:

```ts
interface EventSpec {
  from: EventSource;
  event: string | string[];           // names; "*" for any
  filter?: Expr;                      // `trigger` in scope
  echo?: boolean;                     // (Later) include echoes (surfaces §4.4)
}

type EventSource =
  | { path: Selector<Ref> }                                   // any node(s); {match} for patterns
  | { target: Selector<Target> }                              // connections, items, interfaces
  | { layer: string; controls?: Selector<ControlId> }         // the controls a layer controls, per surface
  | { surface: string; controls?: Selector<ControlId> }       // whatever layer controls them
  | { group: string }                                         // (Later) any control in a control group
  | { slot: string }                                          // (M9) inside a component: one of its slots
  | { controls: Selector<ControlId> };                        // (M9) in a flow a layer owns: that layer's controls
```

- **Every form reduces to a path pattern and a context filter.** `{layer: L, controls: [k0]}` is `{path: {match: "surface.*.controls.k0"}}` where the event's `layer` is `L`. `{target: {interface: "camera.ptz@1"}}` is the roots of every implementer, with names mapped through the interface. The matcher indexes them, so a trigger costs nothing until its events happen.
- **The `trigger` payload is the same for every event:** `{path, event, payload, captures?, source?, surface?, layer?, control?, item?, index?, instance?}`. `source` is the target, for target events. `captures` are the pattern's matches. **(M9)** `instance` is the chain of instances the event came from ([composition §5](composition.md)).
- **A `layer` source matches per surface,** on whichever surfaces that layer controlled the control when the input arrived, as `control` triggers did (surfaces §7).

---

## 4. Transforms

A **transform** is a function over sources, used as `{use: <id>, …options}` anywhere a source can be. Its options are themselves sources, so transforms chain.

```ts
{ use: "map", input: { use: "compare", input: { ref: … }, op: ">", value: 3 }, … }
```

### 4.1 Stateless transforms (M6)

A **stateless** transform computes a value from its sources and nothing else. These replace the binding kinds ([bindings §2](bindings.md)) and add boolean logic.

| `use` | Options | Gives |
|---|---|---|
| `format` | `input: Source`, `format: Format` | text: `auto`, `mmss`, `hhmmss`, `percent`, `fixed1`, `fixed2`, `upper` |
| `template` | `parts: (string \| Source)[]` | text; `undefined` if any part is |
| `rules` | `rules: {when: Source<boolean>, value: Source<T>}[]`, `else: Source<T>` | the first matching value |
| `compare` | `input: Source`, `op: "==" \| "!=" \| ">" \| ">=" \| "<" \| "<=" \| "truthy" \| "falsy"`, `value?: Source` | boolean |
| `all`, `any` | `of: Source<boolean>[]` | boolean (JavaScript truthiness) |
| `not` | `input: Source<boolean>` | boolean |
| `map` | `input: Source<number>`, `from`, `to`, `curve?`, `clamp?` | a number, or an item of a list |

- **Each has a compiled expression,** shown in the editor and used as its specification ([ADR 0023](../adr/0023-key-rendering-and-binding-evaluation.md)): `rules` compiles to a chain of `?:`. They're evaluated natively in Rust, with tests that require the native result to equal the expression's.
- **The rules of [bindings §2.1](bindings.md)** (strict `==`, JavaScript relational comparison, `map`'s formula) carry over unchanged.
- **"Edit as expression" is one-way,** as now.

### 4.2 Reducers (M8)

A **reducer** keeps a value of its own and updates it on events or on its input changing. Reducers replace helpers (surfaces §4.3); because they're sources, they can be used anywhere, not only on a control.

```ts
interface ReducerOptions {
  on?: EventSpec;                     // the events that step it
  input?: Source;                     // a value whose changes step it
  follow?: Source;                    // keep the value in step with this (§4.3)
  holdoff?: Source<Duration>;         // default 250ms
}
```

| `use` | Steps on | Options | Value | Events |
|---|---|---|---|---|
| `toggle` | `on` (default: `press`) | `initial` | boolean, flipped | `on`, `off` |
| `counter` | `on` | `initial`, `step`, `min`, `max`, `wrap` | number | `change` |
| `position` | `input` detents or value | `min`, `max`, `step`, `wrap`, `accel` | number in `[min, max]` | `change` |
| `velocity` | `input` detents or value | `window`, `decay`, `scale` | units per second, decaying to 0 | none |
| `select` | `input` detents | `count` or `items: Source<Value[]>`, `wrap` | an index, or the item | `select` |
| `absolute` | `input` value | `curve`, `pickup` | 0 to 1 through the curve | `change` |
| `rate` | `input` value | `centre`, `deadband` | −1 to 1 | none |
| `latch` | `on` | `set`, `reset` event names | boolean | `on`, `off` |
| `script` | `on`, `input` | `source: Expr` | whatever it returns | whatever it returns |

- **A reducer needs a home.** Its value has to live somewhere for others to read, so it may only appear as a cell's source (§5): a variable, a control binding's helper, a composite's state, a component's helper. Inside a display binding or a step parameter it's `unsupported`.
- **Defaults from the control.** As a control binding's helper, `on` defaults to that control's events and `input` to its input state, so `{use: "toggle"}` is enough for a latching key.
- **Options can follow state:** every option is a `Source`, re-read when its reads change.
- **Time** comes from the scheduler: a reducer gets `now` with each step and can ask to be woken at a time, which is how `velocity` decays. It never reads a clock itself.

### 4.3 `follow`, `holdoff` and `pickup`

- **`follow`** sets the value from a source whenever the source changes, so input continues from where the device is rather than jumping. It pauses while the control is `touched`, and for `holdoff` after the last input.
- **`pickup`** (`absolute` only) ignores input until it crosses the followed value.
- These are the rules [ADR 0024](../adr/0024-surfaces-as-two-way-devices.md) gave helpers, unchanged.

### 4.4 Transform definitions (M8; others' in M9)

Every transform has a **definition**: a pure JavaScript module run in the core's sandbox ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)). The built-in transforms are defined this way first ([ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md)), and plugins and projects add their own with the same format (M9).

```js
export default defineTransform({
  id: "toggle",
  version: 1,
  kind: "reducer",                              // or "stateless"
  options: { initial: { type: "boolean", default: false } },
  steps: { on: ["press"] },                     // default event names, or { input: "number" }
  result: { type: "boolean" },
  events: { on: {}, off: {} },
  init: ({ options }) => options.initial,
  step: (state, { event, input, options, now }) => {
    const next = !state;
    return { state: next, value: next, events: [next ? "on" : "off"] };
  },
});
```

- **`stateless`** definitions have `compute({inputs, options}) → value` instead of `init` and `step`.
- **Pure functions only.** No host API, no clock (`now` is passed in), no randomness, no state between calls except what `step` returns. The core keeps the state. A step can return `wakeAt` (ms, scheduler time) to be stepped again with `event: undefined`.
- **Limits:** the bindings' 10 ms per call and the expression memory limit. A failing step keeps the previous state and value and is reported like a binding error.
- **Native implementations are allowed** for speed, if a conformance suite runs the definition and the native code over the same inputs and requires equal states, values and events. The definition is the specification.
- **Names.** Built-ins are bare (`toggle`). **(M9)** A plugin's are `<plugin id>/<id>` (`io.covalence.streamdeck/ringLevel`), and a project's are `project/<slug>`. Versions follow the interface rule: a breaking change is a new `version`, and a saved `use` may name one (`toggle@1`).

---

## 5. Cells (M8)

A **cell** is a source with a home in the state tree. It's how a computed or accumulated value becomes state that bindings, triggers, flows and other cells can read.

| Cell | Home | Defined by |
|---|---|---|
| A variable (§6) | `vars.<slug>` | the `variable` entity's `value` |
| A control's helper | `layer.<slug>.controls.<id>.<name>`, or with `scope: "surface"`, `surface.<slug>.controls.<id>.helpers.<name>` | a control binding's `helpers` (bindings §5) |
| A composite's state (M9) | `<alias>.<path>`, or per item `<alias>.items[<key>].<path>` | the composite's `state` (composition §6) |

### 5.1 Writing
- **A cell's value is state.** A change is journaled as a `state.patch` with origin `{core: "dataflow"}`, `cause` → the latest entry among its reads that changed (or the event that stepped it), so `why` on anything that read a cell reaches the input that moved it.
- **Read-only.** Only the core writes a cell. A `set` step on one is a save error (`readOnly`).
- **Quality.** A cell is `stale` while any value it read is stale, and `unknown` until it first has a value.
- **High-rate cells** may set `journal: {sampled: "100ms"}`: every value is applied to state, but the journal keeps the latest per interval, with a final entry when it settles. This answers state spec open question 4 for cells and fader input.

### 5.2 Persistence
- **Stateless cells** are recomputed from current state after a restart or a failover.
- **Reducers** restart from `init` unless their home is a variable with `persist: true` (§6), in which case the last value is restored and becomes the state. On failover, the standby has replicated the patches, so a reducer resumes from its last journaled value.

### 5.3 The graph and the scheduler turn
- **Every reactive source is a node** in one graph: cells, display bindings, layer fields, state triggers, `waitUntil` steps and sinks. Edges are read sets, replaced after each evaluation.
- **One turn:**
  1. Apply the turn's patches and events (state spec §1.5).
  2. Evaluate dirty cells in dependency order; a cell's new value dirties its readers in the same turn.
  3. Then, against the settled state: layer resolution (surfaces §5.2), display bindings, state triggers and `waitUntil`, and sinks (§7).
- **Cycles.** A cell that would be evaluated a second time in one turn is on a cycle. Every cell on it keeps its previous value and reports `cycle`, until a later turn breaks the cycle. Saving a definition whose static reads already form a cycle is an error (`cycle`).
- **The budgets stand:** a state change to a redrawn key in under 16 ms, including any cells in between.

---

## 6. Variables (M8)

Variables become a declared entity. The `vars` root stays.

```ts
interface Variable {                  // project entity `variable`
  id: string;
  name: string;
  slug: string;                       // state: vars.<slug>
  schema?: StateSchemaNode;           // type, unit, kind (a `target` with an interface…)
  initial?: Value;                    // a writable variable's value at load
  persist?: boolean;                  // keep the value across a restart; default false
  value?: Source<Value>;              // a computed or reducer variable: read-only
  to?: Sink[];                        // §7
  journal?: "full" | { sampled: Duration };
}
```

- **Writable** (no `value`): `set` steps and scripts write it, checked against `schema`.
- **Computed** (`value` is a stateless source) or **accumulated** (`value` is a reducer): a cell (§5), read-only.
- **Undeclared variables still work:** a `set` to a name with no entity creates the value, with a warning (`undeclaredVariable`). The editor offers to declare it.
- **`persist: true`** keeps the value in `state_cache` and restores it at startup, before any trigger is armed.

---

## 7. Sinks (M8)

A **sink** is where a value goes. Display binding properties are sinks into a control's outputs (bindings §1). `to` adds the other two to any cell:

```ts
type Sink =
  | { set: Ref }                                          // a writable variable
  | { action: {
      target: Source<Target>;
      constraint?: TargetConstraint;                       // when the target isn't a literal
      action: string;
      param: string;                                       // the parameter that receives the value
      params?: { [k: string]: Source<Value> };             // the others
      rateLimit?: Duration;                                // default 20ms; the latest value wins
    } };
```

- **A sink fires when its cell's value changes,** not on every turn and not at load. A `toggle` with `to: {action: {…, param: "on"}}` sends one action per flip.
- **`undefined` sends nothing,** so a `rules` source with no `else` value only sends while a rule matches.
- **Only the latest value is kept** while a rate limit waits, so a slow device never builds a backlog.
- **Actions from sinks** go through the output gate and the journal like any other, with origin `{core: "dataflow"}` and `cause` → the cell's patch.
- **No echo loops:** a sink doesn't fire for a change that descends, through `cause`, from its own previous action. A fader following a mixer's volume and sending it back stays quiet. The flows' loop guard (flows §8.6) is the backstop.
- **Sinks are for keeping things in step.** "When X, do A then B" is a flow.

---

## 8. Scope (M9 for `params`, `self` beyond layers, and `forEach`)

What an expression or `{scope}` ref can name depends on where it's evaluated. **Nothing is inherited implicitly:** a container that needs a value from around it declares a parameter, and whoever places it binds that parameter, the way a function takes arguments.

| Name | In | Is |
|---|---|---|
| state roots | everywhere | `<alias>`, `vars`, `sys`, `surface`, `layer`, `flow` |
| `trigger`, `input`, `local`, `steps`, `error`, loop `<as>` and `index` | flows | flows §3 |
| `params` | the body of a template: a layer, a composite, a component, a flow, and the flows they own | the **own** instance's parameters ([composition §1](composition.md)). In a nested instance, the innermost one's. A free-standing flow sees only its own |
| `self` | a layer's fields | the surface being resolved (surfaces §5.2) |
| `self` | a control binding, a helper, a placement | `{surface, layer, control}`: where it's being evaluated |
| `item`, `index`, `count`, `key` | a control group | the item the control shows (surfaces §6) |
| `item`, `index`, `key` | an instance's `params` and placement fields, when it has `forEach` | the item this instance is for ([composition §4](composition.md)). Not inside the body |

- **`params` replaces the `layer.params.<name>` shorthand** of [bindings §6](bindings.md), which collided with a layer whose slug is `params`. The absolute path is `layer.<slug>.params.<name>`.
- **Placing binds parameters in the placer's scope.** A component placed on a layer gets `params: {camera: {ref: {scope: "params", path: ["camera"]}}}`, which reads the layer's `camera`; inside the component, `params.camera` is its own. The same holds at every level of nesting, and for every template kind ([ADR 0032](../adr/0032-one-template-primitive.md)).

---

## 9. Open questions

1. Should a transform be able to read the previous value of any source (`previous(x)`), or is that always a reducer?
2. Should `Selector<Entities>` get tags (`{tag: "foh"}`) for surfaces and layers, and are tags their own entity?
3. Should sinks support `when` (only send while armed), or is a `rules` source that gives `undefined` (which sends nothing) enough?
4. Is 16 levels of cell depth a sensible limit, or should depth only be bounded by the turn budget?
