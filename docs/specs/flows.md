# Spec: Flows

- **Status:** Draft v0.2
- **Related:** [dataflow](dataflow.md), [core actions](core-actions.md), [composition](composition.md), [state and event log](state-and-event-log.md), [bindings](bindings.md)

A flow is the single unit of automation: triggers, optional conditions, and a body of steps. Button action lists, rules and node graphs are all flows. Flows are **control flow only**. Values move through expressions, not edges (§11.1). Keeping something continuously in step with state is dataflow, not a flow ([dataflow spec](dataflow.md)).

**Markers** in §1–§7 follow the [dataflow spec](dataflow.md): **(M6)**, **(M8)** and **(M9)** name the milestone that implements a rule. §8 lists what's implemented.

Shapes use TypeScript notation for readability. The real source of truth will be the schema in `protocol/`.

---

## 1. The flow entity

```ts
interface Flow {
  id: string;                         // ULID
  name: string;
  slug: string;                       // for state paths: flow.<slug>.running
  enabled: boolean;

  triggers: Trigger[];                // any one fires the flow
  conditions?: Expr[];                // all must be true when a trigger fires
  concurrency: "parallel" | "queue" | "restart" | "ignore";
  failover: "abort" | "resume" | "restart";       // state spec §7.4

  inputs?: { [name: string]: ParamDef };          // (M9) bound per run: call-flow, API, OSC and composite actions
  outputs?: { [name: string]: StateSchemaNode };  // (M9) returned by `stop` (§2.3) to a caller

  params?: { [name: string]: ParamDef };          // (M9) a template (composition §9); or, with `template`, an instance's bindings
  template?: string;                              // (M9) an instance of a flow template; it then has no triggers or body of its own
  forEach?: ForEach;                              // (M9) one instance per item (composition §4)
  owner?: string;                                 // (M9) a layer or composite that owns it (composition §2)

  body: Step[];                       // a sequence
  editor?: EditorMeta;                // view-only metadata (§6); ignored by the runtime
}

type Expr = string;                   // JS expression source (§11.2)
```

- `concurrency` decides what happens when a trigger fires while a run is already active:
  - `parallel`: start another run
  - `queue`: start it after the active run finishes
  - `restart`: cancel the active run, then start
  - `ignore`: drop the new trigger and journal it as ignored
- `conditions` are checked when the trigger matches. If any is false, no run starts, and the journal records `trigger.matched` with `blocked: true`.

### 1.1 Triggers

| Kind | Fields | `trigger` payload in expressions |
|---|---|---|
| `event` | `from: EventSource`, `event: string \| string[]`, `filter?: Expr`, `echo?` ([dataflow §3.1](dataflow.md)) | `{path, event, payload, captures?, source?, surface?, layer?, control?, item?, index?}`, with `payload` typed by the event's definition |
| `state` | `when: Expr \| Source<boolean>`, `edge: rising \| falling \| change` | `{value, previous}` |
| `state` with `match` | `match: PathPattern`, `when?: Expr`, `edge` | `{path, captures, value, previous, ts}` |
| `schedule` | per [state spec §7.7](state-and-event-log.md#77-time-sources-and-scheduling-proposed) | `{scheduled, fired}` |
| `call` | none; any flow may call this one | `{caller: {flow, run}}` |
| `api` / `osc` | `address?` for OSC | `{args}` |
| `action` **(M9)** | none: set by a composite whose action names this flow ([composition §6.2](composition.md)) | `{conn, call}`; the action's params are `input`, and `params` are the composite instance's |

- **(M6) One trigger kind for every event** ([dataflow §10.2](dataflow.md#102-events-are-addressed-by-state-path)). Events happen at a node in the state tree and have a name (`pp:timerEnded`, `surface.deck.controls.k3:press`, `flow.preService:finished`, `sys:startup`). `from` picks the nodes with a path selector, a target selector, a layer or surface with a control selector, a group, or a component slot. The M5 `control` and the planned `startup` kinds are event triggers now: `{from: {layer, controls}, event: "press"}` and `{from: {path: "sys"}, event: "startup"}`.
- An `event` trigger can listen to many targets instead of one connection. `from: {target: {template: <Camera's ID>}}` fires for every instance of a composite template, and `{target: {plugin: …}}` for every connection of a plugin; `trigger.source` says which ([plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed)).
- `state` triggers are edge-triggered. Their dependencies are tracked at run time, like bindings. **(M6)** `when` can be a structured source (`{use: "compare", …}`), as well as an expression string.
- **A control's input is state, and its events are derived from that state** ([surfaces §11.2](surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state), [surfaces spec §4](surfaces.md)). An event trigger is the quick way in for them: a press, a double, a turn. A layer source matches per surface, on whichever surfaces that layer controls the control.
- **(M6) Flows emit events** at `flow.<slug>`: `started`, `finished` and `failed`, with `{run, status, error?}`. "When that flow finishes" is an event trigger.
- **(M9) `params`** is the flow's own instance's parameters, or for an owned flow its owner instance's ([composition §2](composition.md)). A free-standing flow triggered by a control event doesn't get the layer's: it gets `trigger.layer` and `trigger.instance`, and a flow that should handle every instance of a layer template is owned by it ([composition §11.2](composition.md#112-one-template-primitive-for-four-kinds)).
- **`state` triggers cover conditions and combinations** over input state: a press that only counts while armed, a shift key, a helper's position crossing a threshold.
- **A `state` trigger with `match`** fires for each path matching the pattern, with `*` matching one segment: `surface.*.controls.*.pressed`. `trigger.path` is the path that changed, and `trigger.captures` the segments `*` matched. `when` is optional, and sees `trigger.value`. It works on any state, so a GPIO contact from a plugin and a key on a surface can drive the same flow.

---

## 2. Steps

Every step has these common fields:

```ts
interface StepBase {
  id: string;            // unique within the flow; [a-z][A-Za-z0-9_]*; shown in the UI and used in `steps.<id>`
  kind: string;
  name?: string;         // display label; defaults from the kind and parameters
  enabled?: boolean;     // false: journaled as skipped
  timeout?: Duration;    // the step fails with `timeout` after this long
  retry?: { attempts: number; backoff?: Duration };   // leaf steps only
  onError?: "fail" | "continue";                      // default "fail": the error moves up to the enclosing block
}

type Duration = string;  // "500ms", "2s", "1m30s"
```

- **Step IDs are generated** by the editor (`route1`, `wait2`) and can be renamed. Renaming rewrites every reference to that step in the same flow. That's safe because the core parses all expressions.
- `onError: "continue"` records the error in `steps.<id>` and moves on.

### 2.1 Leaf steps

| Kind | Fields | Result (`steps.<id>.result`) |
|---|---|---|
| `action` | `target: Source<Target>`, `constraint?`, `action`, `params: {[k]: Source<Value>}` | The action's `result`, per its `ActionDef` |
| `wait` | `for: Source<Duration>` | none |
| `waitUntil` | `until: Expr`, `timeout?` | `{value}`. Checked immediately (level, not edge), then on every change |
| `call` | `flow`, `inputs?`, `wait?: boolean` (default `true`) | The callee's `outputs`, or `{run}` if `wait` is `false` |
| `set` | `path: "local.<name>" \| "vars.<name>"`, `value: Expr` | none |
| `script` | `source` (JS/TS), `timeout?` | Whatever the script returns |
| `log` | `level`, `message: Expr` | none |
| `stop` | `status?: ok \| failed`, `outputs?`, `error?` | ends the run |
| `break` / `continue` | none | only inside a `loop` |

```ts
type Target = { conn: string };      // a connection; what's within it is a parameter (plugin protocol spec §13.9)

// An action step's target is a Source (dataflow §1): a literal Target is fixed;
// a ref or an expression is dynamic, and then `constraint` is required.
type TargetConstraint =
  | { plugin: string }                // a plugin's connections
  | { template: string };             // (M9) every instance of a composite template (composition §6.5)
```

- **A target that follows state is resolved when the step runs.** Its value is a connection ID or `{conn}`, as state holds it, or an alias, as an expression may write it. It has to be a connection of the constraint's plugin; anything else, or `undefined`, fails the step with `no_target` and nothing is requested. On save, the action and its literal parameters are checked against the plugin's manifest, from any of its connections that has loaded; with none loaded, that's an `unchecked` warning. A literal target with a constraint ignores the constraint.
- **(M6) Parameters are `Source`s,** one per top-level parameter: a literal, `{ref}`, `{expr}` or a stateless transform. This generalises M4's `{expr}` parameters (§8.3), which are already in this shape.
- **(M6) The core's own actions** (navigate a surface, inject input, cancel a flow's runs) are actions on the `core` connection ([core actions](core-actions.md)), with no step kinds of their own.

### 2.2 Blocks

| Kind | Fields | Semantics |
|---|---|---|
| `group` | `steps`, `catch?: Step[]`, `finally?: Step[]` | Runs `steps` in order. If one fails, runs `catch` with `error` in scope. Always runs `finally`, including on cancel |
| `branch` | `arms: {when: Expr, steps}[]`, `else?: Step[]` | Runs the first arm whose `when` is true |
| `loop` | `count: Expr` **or** `while: Expr` **or** `forEach: Expr`, `as?` (default `item`), `steps`, `max?` | `forEach` exposes `<as>` and `index`. `max` (default 10,000) guards against runaway loops |
| `parallel` | `branches: Step[][]`, `join: all \| first \| any` | See below |

`parallel` join modes:
- `all`: wait for every branch. If one fails, cancel the others and fail.
- `first`: the first branch to finish successfully wins and the others are cancelled. Fails only if all branches fail.
- `any`: wait for every branch, collect each one's outcome, and never fail because of a branch.

A block's result is `{status}`, plus `{branches: [{status, error?}]}` for `parallel`.

### 2.3 Ending a run

A run ends when its body finishes, a `stop` step runs, an error reaches the top of the body, or it's cancelled. `stop` inside a called flow ends only that flow and returns its `outputs` to the caller.

---

## 3. Data inside a run

Steps read data through expressions. These names are in scope:

| Name | Contents | Writable |
|---|---|---|
| `trigger` | The trigger payload (§1.1) | no |
| `input` | The flow's inputs, bound for this run | no |
| `local` | Run-local variables. They last for the run and are included in checkpoints | via `set` and scripts |
| `steps.<id>` | `{status, result?, error?, startedAt, durationMs}` for steps that have run | no |
| `<as>`, `index` | The current loop item and index, inside `forEach` | no |
| `error` | The error, inside a `catch` | no |
| state roots | `<alias>`, `vars`, `sys`, `surface`, `layer` (M5: `page`), `flow` ([state spec §1.1](state-and-event-log.md)) | `vars` via `set` and scripts |
| `target(t)` | A target's state ([plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed)) | no |
| `params` **(M9)** | The flow instance's parameters, or its owner instance's for an owned flow: a layer's, a composite's or a placement's ([composition §2](composition.md)) | no |

- **Steps in a branch that didn't run are absent** from `steps`. Reading them gives `undefined`, like any missing value ([§11.2](#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)).
- **Inside a `parallel` block,** a branch can only see its own steps and those that finished before the block started. Other branches' steps become visible after the join.
- **Scripts** get the same scopes as globals, plus the host API from [§11.2](#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs). `setLocal(name, value)` joins `setVar`.

---

## 4. Typing

Types are checked **when editing**. Values stay dynamic at runtime.

- The editor generates TypeScript definitions for each flow from:
  - the trigger kind and the `EventDef` payload schema
  - the flow's `inputs`
  - each action's `result` schema, for `steps.<id>.result`
  - `local` names, inferred from `set` steps. A `local` may also be declared with a schema in `editor.locals`
  - the state schema and plugin manifests, as in [§11.2](#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)
  - for a dynamic target, the constraint's actions: the plugin's manifest or the composite template's actions
- Action `params` are checked against the action's schema, taken from the target's constraint when the target is dynamic.
- **Errors block saving. Warnings don't.**
  - Errors: unknown action, missing required parameter, type mismatch, a `break` outside a loop, a reference to a step ID that doesn't exist.
  - Warnings: a state path that isn't in the schema yet (it may appear after a connection connects), and possibly-`undefined` reads.

---

## 5. Execution

### 5.1 Position and checkpoints
- A run's **position** is a path of step IDs from the body down to the current step, for example `[intro, camLoop#2, recall]`. Loop iterations are written as `#n`. A `parallel` block has one position per branch.
- `run.checkpoint` stores `{run, positions, locals, steps}` ([state spec §2.2](state-and-event-log.md)). For `resume` on failover, the new active node restarts each position at the start of the step it names. Steps whose actions aren't `idempotent` are skipped and marked `uncertain` rather than run twice.

### 5.2 Cancellation
- Cooperative, at every step boundary and inside scripts (QuickJS interrupt hook).
- Cancelling a block cancels its children, deepest first. A running action gets `action.cancel` if it's `cancellable`.
- `finally` steps run during cancellation, with a fixed time limit (default 5 s). They can't be cancelled again, except by that limit.

### 5.3 Journaling
- Each step logs `run.step` `started`, then `done`, `failed` or `skipped`. `detail` carries the step's position and, on failure, the error.
- Actions log `action.requested` with `cause` → the `run.step` entry. A dynamic target also records the resolved `conn` ([plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed)).
- `retry` attempts each journal a `run.step` entry, with `attempt` in `detail`.

### 5.4 Flow status in state
`flow.<slug>.running` (count of active runs), `flow.<slug>.lastRun.{status, startedAt, durationMs, error?}`.

**(M6)** A run also emits events at `flow.<slug>`: `started` with `{run}`, then `finished` with `{run, status}` and, when it failed, `failed` with `{run, error}`. They're journaled as `event` with `cause` → `run.started` or `run.finished`.

---

## 6. Views

The three views are three ways of showing the same tree.

| View | Shows a flow when | Notes |
|---|---|---|
| **Button** | At least one event trigger on controls, no conditions, and a body of only `action`, `wait`, `set` and `log` steps | Edits add steps to the flat list. **(Later)** Other triggers show as "Also runs when…" chips, and editing a flow with more than one trigger says so first |
| **Rule** | One trigger, and a body that is either the Button shape or a single `branch` whose arms have that shape | Conditions show as the "if" card |
| **Flow** | Any flow | Blocks are nested regions. Edges show order only |

- **A key's flows.** A control's binding holds its look and its helpers; what happens when it's used lives in flows ([surfaces §11.2](surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state)). **(Later)** Selecting a key in the surface editor lists every flow that references it (by layer, control group or region), with each flow's other triggers, and opens them in the Button view.
- "Open as flow" is always possible. Going back down is offered when the tree matches the simpler shape.
- `editor` holds view-only data: node positions, collapsed blocks, colours and comments. It's exported, so a layout survives a round trip, but it never affects a run and its changes are left out of the diff summary.
- Without saved positions, the Flow view lays the tree out automatically from top to bottom.

---

## 7. Text export

Flows export as YAML ([state spec §7.2](state-and-event-log.md#72-storage-sqlite-a-text-export-migrations)), one file per flow. Connections are stored as IDs and shown with their alias as a comment.

```yaml
id: 01JBZ8X4M2K9Q7N3T5V6W8Y0A1
name: Recall shot on selected camera
slug: recallShot
enabled: true
triggers:
  - kind: event
    from: { layer: 01JBZ9CAMCONTROL0LAYER00000, controls: k8 }   # camControl
    event: press
concurrency: restart
failover: abort
body:
  - id: recall
    kind: action
    target: { ref: { root: vars, path: [selectedCam] } }
    constraint: { template: 01JC0CAMERA0TEMPLATE0000000 }   # camera
    action: recallPreset
    params: { preset: { expr: params.shot } }
    timeout: 2s
    retry: { attempts: 2, backoff: 200ms }
  - id: settle
    kind: waitUntil
    until: "!target(vars.selectedCam).presets.recalling"
    timeout: 5s
    onError: continue
  - id: cut
    kind: action
    target: { conn: 01JBZ7P0D3F5G7H9J1K3L5M7N9 }   # atem
    action: cut
    params: {}
```

Keys appear in a fixed order. Fields that are always expressions (`when`, `until`, `while`, `filter`, `value`) hold plain strings. Action parameters and targets are `Source`s ([dataflow §1](dataflow.md)): literal values unless they're `{ref}`, `{expr}` or `{use}`, so a string parameter is never mistaken for code.

---

## 8. The implemented subset

This section is what the core accepts today.
- [Milestone 3](../milestones/m3-first-flows.md) implemented the Button-view shape (§6) without expressions.
- [Milestone 4](../milestones/m4-expressions.md) adds expressions (§8.7), `state` triggers, `event` filters, flow conditions, and the `branch`, `waitUntil` and `set` steps. Rules that M4 adds or changes are marked **(M4)**.
- [Milestone 5](../milestones/m5-surfaces-and-bindings.md) adds `gesture` triggers, marked **(M5)**. M5 Phase 5 replaced them with `control` triggers on input events ([surfaces §11.2](surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state)), marked **(Now)** and implemented; the **(M5)** `gesture` rows describe what's gone. Rules marked **(Later)** have no milestone yet.
- [Milestone 6](../milestones/m6-primitives.md) replaces `control` triggers with `event` triggers whose `from` is an event source, adds pattern `state` triggers and flow events, and makes action targets and parameters `Source`s. Rules it adds or changes are marked **(M6)**, and aren't implemented yet.

**Anything outside this section is rejected on save with an error, never stored and ignored.** Types live in `covalence_core::flow` ([overview: project entities](overview.md#project-entities-are-rust-types)).

### 8.1 Flow fields

| Field | Accepted |
|---|---|
| `id` | Assigned by the server on create (ULID). Ignored in a create body. Must match the URL on update |
| `name` | Required, non-empty |
| `slug` | Optional on create: derived from `name` (camelCase, ASCII, deduplicated with a number). Must match `[a-z][A-Za-z0-9_]*` and be unique |
| `enabled` | Required |
| `triggers` | `api`, `call`, `event`, **(M4)** `state` and **(M5)** `gesture`, **(Now)** `control` (§8.2), which **(M6)** folds into `event`. May be empty, with a warning: the flow can't start |
| `conditions` | **(M4)** Optional list of expressions. All must be truthy when a trigger matches, or no run starts (§8.2) |
| `inputs` | Optional. `{[name]: ParamDef}`: a state schema node (`type`, `minimum` …, checked as action params are) with `default?` (a source, evaluated when the run is fired) and `required?`. Names are identifiers. Steps read the bound values as `input.<name>` (§3) |
| `outputs` | Optional. `{[name]: StateSchemaNode}`: what a `stop` step may hand back |
| `concurrency` | Required. All four modes |
| `failover` | Optional; only `abort` (the default) |
| `body` | `action`, `wait`, `call`, `stop` and **(M4)** `waitUntil`, `set` and `branch` steps (§8.3). `branch` is the only block |
| `editor` | Stored as-is, never read by the runtime |

### 8.2 Triggers

| Kind | Fields | `trigger` in expressions | Notes |
|---|---|---|---|
| `api` | none | `{args: {}}` | Fired by `POST /flows/{id}/run` (§9). A flow without one can't be run from the API or the UI's Run button |
| `event` | `source: {conn}`, `event`, **(M4)** `filter?: Expr` | `{source, event, payload}` | `event` must be in the plugin manifest's `events`. No `{plugin}` or `{interface}` sources. **(M6)** `from: EventSource` replaces `source` (`{conn}` becomes `{target: conn}`), with path, target, layer and surface sources, `event` as a name or a list, and the payload of dataflow §3.1 |
| `state` **(M4)** | `when: Expr`, `edge: rising \| falling \| change` | `{value, previous}` | See below |
| `gesture` **(M5)** | `control: {page, control}`, `gesture: press \| release \| double \| long \| hold` | `{surface, page, control, gesture, delta?}`, with IDs | Matches on any surface showing the page. `turn` isn't supported yet. Gestures and their timing are in the [surfaces spec §4.2](surfaces.md). `trigger.matched` has `cause` → the `input.gesture` entry |
| `control` **(Now)** | `control: {layer, control}`, `event: press \| release \| double \| long \| hold \| turn \| change \| touch \| untouch` | `{surface, layer, control, event, delta?, value?}`, with IDs | Replaces `gesture`. Matches the control on any surface where the layer controlled it. `trigger.matched` has `cause` → the `input.event` entry ([surfaces spec §7](surfaces.md)). **(M6)** Replaced by `event` with `from: {layer, controls}`; stored flows are migrated |

- `call` triggers have no fields: a flow with one can be started by a `call` step in any flow (§8.3). `trigger` is `{caller: {flow, run}}`.
- **Inputs are bound when a trigger fires.** `call` and `api` give them: each given value is checked against its `ParamDef`, the rest take their defaults, and a required input without a default that isn't given refuses the start (`missingInput`). Every other trigger binds only the defaults, so a flow with a required input that has no default can't have `event` or `state` triggers (§8.4). Conditions and filters don't see `input`.
- `trigger.matched` names the trigger by its index in `triggers`.
- **(M4) Matching order.** When a trigger fires:
  1. Its `filter` is evaluated, if it has one. A falsy result journals nothing, because filters run on every event.
  2. `trigger.matched` is journaled.
  3. The loop guard is checked (§8.6). If it blocks, the reason is `loop`.
  4. The flow's `conditions` are evaluated in order. The first falsy one blocks with reason `condition`.
  5. The concurrency mode is applied, which may block with `ignore` or `queueFull`.

  A filter or condition that fails (§8.7) counts as falsy, and the failure is logged, rate-limited.
- **(M4) `state` triggers.**
  - **Baseline.** `when` is evaluated when the flow is loaded: at startup, on save, and when it's enabled. That first value is the baseline, and it never fires.
  - **Re-evaluation.** After that, `when` is re-evaluated once per scheduler turn in which a path in its read set changed (§8.7).
  - **Edges.**
    - `rising` fires when the value goes from falsy to truthy.
    - `falling` fires when it goes from truthy to falsy.
    - `change` fires when the value differs from the previous one, compared as `Value`s (deep equality).
  - **Failures.** A failed evaluation counts as `undefined`, and the failure is logged, rate-limited.
  - **Payload.** `trigger.value` and `trigger.previous` are the new and old results.
  - **Cause.** `trigger.matched` has `cause` → the last `state.patch` in that turn that touched the read set.
  - **When it doesn't run.** A disabled flow doesn't evaluate its `state` triggers. Saving a flow resets their baselines.
  - **(Now) Input state.** Key, touch and encoder input each get a scheduler turn of their own (surfaces spec §4.1), so a `state` trigger over `pressed` sees every press, however short. Its `cause` is the input's `state.patch`.

### 8.3 Steps

Common fields: `id`, `kind`, `name?`, `enabled?`, `timeout?`, `onError?`. `retry` is rejected. Step IDs are unique across the whole flow, including steps nested in `branch` arms.

| Kind | Fields | Result (`steps.<id>.result`) | Notes |
|---|---|---|---|
| `action` | `target: {conn}`, `action`, `params` | The action's result | A fixed target. **(M4)** Each top-level parameter is a literal value or `{expr: Expr}`. See below |
| `wait` | `for: Duration` or **(M4)** `{expr: Expr}` | none | The expression must give a duration string or a number of milliseconds ≥ 0. Anything else fails the step with `invalidDuration` |
| `waitUntil` **(M4)** | `until: Expr`, `timeout?` | `{value}` | Evaluated when the step starts, and done straight away if truthy (level, not edge). Otherwise re-evaluated whenever its read set changes, until truthy. `timeout` fails it with `timeout` |
| `set` **(M4)** | `path: "local.<name>" \| "vars.<name>"`, `value: Expr` | none | `<name>` is one segment, `[A-Za-z_][A-Za-z0-9_]*`. See below |
| `branch` **(M4)** | `arms: {when: Expr, steps: Step[]}[]`, `else?: Step[]` | `{status}` | See below |
| `call` | `flow` (an ID), `inputs?: {[name]: Source}`, `wait?: boolean` (default `true`) | The callee's outputs (`{}` if it hands back none), or `{run}` without `wait` | See below |
| `stop` | `status?: "ok" \| "failed"` (default `ok`), `outputs?: {[name]: Source}`, `error?: Expr` | none: the run ends | See below |

- **Durations** match `^(\d+h)?(\d+m)?(\d+s)?(\d+ms)?$`, are not empty, and are greater than zero: `500ms`, `2s`, `1m30s`. A literal `wait` must be greater than zero. An expression may give zero, which doesn't wait.
- **An action step** fails if the action returns an error (with that error), if the output gate suppresses it (code `suppressed`, with the reason), or if its connection isn't running (code `unavailable`). Its `result` is the action's result.
- **(M4) Expression parameters.**
  - **Recognising them.** A parameter value whose only key is `expr`, holding a string, is an expression. That's checked only at the top level of `params`. Inside a literal object or array, `{expr: ...}` is a literal, so a parameter that needs such an object has to be built by an expression.
  - **Evaluation.** Every expression parameter is evaluated when the step starts. The whole `params` object is then checked against the action's schema.
  - **Failures.** A failed expression fails the step with its expression code (§8.7). A schema failure fails it with `invalidParams`, and the error includes the evaluated `params`.
- **(M4) `set`.**
  - **Evaluation.** `value` is evaluated when the step starts.
  - **`local.<name>`** is visible to later steps in the same run, and is gone when the run ends.
  - **`vars.<name>`** is written to the state store as a `state.patch` with origin `{flow, run, step}` and `cause` → the step's `run.step started` entry. Variables aren't declared and aren't kept across a restart ([M4](../milestones/m4-expressions.md)).
  - **Removing.** A value of `undefined` removes the local or variable.
- **(M4) `branch`.**
  - **Choosing an arm.** The arms' `when` expressions are evaluated in order, and the steps of the first truthy arm run. If none is truthy, `else` runs. With no `else`, the branch is done without running any steps.
  - **Failures.** A `when` that fails fails the branch. An error from a step in the chosen arm moves up to the branch, where the branch's own `onError` applies.
  - **Timeout.** A branch's `timeout` covers the whole block.
  - **Nesting.** A branch may be nested inside another branch. It needs at least one arm.
- **`call`.**
  - **Starting it.** The inputs are evaluated when the step starts, then the callee's `call` trigger fires with `cause` → the step's `run.step started`, so `why` on anything the callee does walks back through the caller. The callee's own concurrency applies: `queue` waits its turn, `restart` cancels its active run, and `ignore` while it's busy fails the step with `busy`. Other failures before it starts: `unknownFlow`, `disabled`, `notCallable` (no `call` trigger), `unknownInput`, `missingInput`, `invalidInputs`, `queueFull`, `condition`, and `loop` when the callee is already in the chain of calls (§8.6), which is what stops a flow calling itself.
  - **Waiting.** With `wait`, the step ends when the callee's run does. `ok` gives its outputs as the result; `failed` fails the step with the callee's error code and the message `flow <name> failed: <message>`; a callee cancelled by someone else fails it with `callCancelled`. Cancelling the caller, or the step's `timeout`, cancels the callee and waits for it to end.
  - **Without `wait`** the step is done as soon as the run is started or queued, and the two runs are independent.
- **`stop`** ends the run once it's journaled `done`, from any depth: a `stop` in a branch ends the branch and the run. Each output is evaluated and checked against the flow's `outputs` (`unknownOutput`, `invalidOutputs` fail the step instead). `status: "failed"` fails the run with code `stopped` and `error`'s value as the message. The outputs are recorded on `run.finished` and the run (§9), and a caller takes them as its step's result.
- **A `timeout`** fails the step with code `timeout`. A late `action.result` is still journaled but no longer affects the run.
- **`enabled: false`** journals the step as `skipped`. A skipped `branch` skips its arms without journaling them.

### 8.4 Validation

Errors block the save; warnings don't.

- **Errors:**
  - anything outside §8.1–8.3
  - a duplicate or malformed step ID
  - a duplicate slug
  - an unknown connection
  - an action or event the connection's plugin doesn't declare
  - literal parameters that fail the action's schema
  - a bad literal duration
  - **(M4)** an expression that doesn't compile, a malformed `set` path, a `branch` with no arms
  - a `call` step on a flow that doesn't exist (`unknownFlow`) or has no `call` trigger (`notCallable`), an input the callee doesn't declare (`unknownInput`), or a required input with no default left out (`missingInput`); an `event` or `state` trigger on a flow with such an input (`missingInput`); a `stop` output the flow doesn't declare (`unknownOutput`); an input or output name that isn't an identifier (`badId`); a literal input, default or output that fails its schema (`badValue`)
  - **(M5)** a `gesture` trigger on a page that doesn't exist (`unknownPage`), or a control outside its layout (`unknownControl`). **(Now)** The same for a `control` trigger, with `unknownLayer`, `unknownControl` for a control the layer doesn't list, and `unsupported` for an event its control's inputs can't give
- **Warnings:**
  - no triggers
  - an `event` trigger on a connection the flow also acts on (see §8.6)
  - a connection whose plugin hasn't loaded yet, so its actions and events couldn't be checked
  - **(M4)** a `waitUntil` without a `timeout`
- **(M4) What the core doesn't check.**
  - **Parameter schemas, for any parameter that is an expression.** Its value is checked at run time, along with the rest of `params`.
  - **References to steps** (`steps.route9`). The editor warns about those, and at run time they read as `undefined` ([§11.2](#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)).
- **Problem shape.** Each problem is `{path, code, message, at?}`.
  - `path` is a segment array into the flow, e.g. `["body", 1, "params", "input"]`.
  - **(M4)** For an expression that doesn't compile, the code is `exprSyntax` and `at` is `{line, column}` within the expression's source.
- Stored flows aren't revalidated at startup. If a connection disappears, its steps fail at run time with `unavailable`.

### 8.5 Runs

- **A run** has a ULID, a status (`queued`, `running`, `ok`, `failed`, `cancelled`), and a copy of the flow taken when it started. Editing or disabling a flow doesn't touch runs already active. Deleting it cancels them. **(M9, proposed)** `run.started` records the flow's revision, and an edit never changes a run already active ([state spec §7.6](state-and-event-log.md#76-editing-a-running-show-proposed-kept-state-settled-in-m8)).
- **Concurrency.**
  - `queue`: a queued trigger gets its run ID straight away but journals nothing more until it starts. The queue holds at most 16 runs. Past that, the trigger is blocked with reason `queueFull` (§10 question 2).
  - `restart`: cancels the active run and starts the new one once the old run has journaled `run.finished`.
- **Cancellation** happens at step boundaries, during a wait or **(M4)** `waitUntil`, and **(M4)** inside an expression's evaluation.
  - **A cut-short wait.** A `wait` or `waitUntil` that is cut short journals its step `failed` with code `cancelled`.
  - **An action in flight** is waited for: its step ends `done` or `failed`, then the run ends `cancelled`.
  - **A queued run.** Cancelling it removes it and journals `run.finished cancelled`.
  - **(M4) Inside a branch.** Cancelling a run inside a `branch` fails the running step, then the branch, with `cancelled`.
- **(M4) Run data.** Each run keeps `local` and `steps` (§8.7) in memory for as long as it runs. Neither is journaled or checkpointed yet.
- **Journal entries for a run** carry `run` in the envelope and origin `{flow, run, step?}` (`step` only on step-level entries). Causes:

  | Entry | `cause` → |
  |---|---|
  | `trigger.matched` | the `event` entry (origin is the core); **(M4)** for a `state` trigger, the `state.patch` entry (§8.2); none for `api` (origin is the user) |
  | `trigger.matched` | for a `call` trigger, the caller's `run.step started` |
  | `run.started` | `trigger.matched` |
  | `run.step` | `run.started` |
  | `action.requested` | that step's `run.step started` |
  | `state.patch` **(M4)** | from a `set` step, that step's `run.step started` |
  | `run.finished` | `run.started` (or `trigger.matched` for a queued run that never started) |

- `run.started` data is `{flow, run, mode, inputs}`, with `mode` the flow's concurrency mode and `inputs` the bound inputs. `run.finished` has `outputs` when a `stop` step handed some back.
- **`run.step` `detail`** is `{position, arm?, error?}`.
  - `position` is the path of step IDs from the body down to the step: `[route1]` at the top level, and **(M4)** `[branch1, route2]` inside an arm.
  - **(M4)** A `branch`'s `done` entry carries `arm`: the index of the chosen arm, `"else"`, or `null` when nothing ran.
- `trigger.matched` gains a `reason` when `blocked`: `ignore` (concurrency), `queueFull`, `loop` (§8.6), or **(M4)** `condition`, with `condition` the index of the first falsy one.
- **State:** `flow.<slug>.running` and `flow.<slug>.lastRun.{status, startedAt, durationMs, error?}` (§5.4). `startedAt` is milliseconds since the epoch, like a leaf's `ts`. Deleting a flow deletes its subtree.
- **The core keeps** every active and queued run plus the last 100 finished runs in memory, for `GET /runs`. The journal is the record; the list is a convenience and is empty after a restart.

### 8.6 Loop guard

A flow that listens to an event on a connection and also acts on that connection can re-trigger itself forever. Two flows can do the same to each other. **(M4)** So can a flow whose `state` trigger reads what its own steps write.

- **At run time, a trigger is blocked with reason `loop`** when the entry that fired it descends, through `cause`, from a run of the same flow. That catches self-loops and loops through other flows. **(M4)** It covers `state` triggers too, including patches written by `set`. The check follows the chain only back to its first `run.started` of this flow, so it stays short.
- **On save,** an `event` trigger on a connection the flow's own steps act on is a warning (§8.4).
- **Known gap:** a live device's echo often arrives without a `cause` ([M1 known gaps](../milestones/m1-walking-skeleton.md#known-gaps-carried-forward)), so the guard can't see live loops. **(M4)** A `filter` that excludes the flow's own change (for example, the output it routes) breaks the loop. The guard alone still doesn't catch it.

### 8.7 Expressions (M4)

Why the language and sandbox are this way: §11.2.

**Where they appear:** `{expr}` action parameters, `wait.for`, `event` `filter`, flow `conditions`, `state` `when`, `waitUntil` `until`, `set` `value`, and `branch` `when`.

**Form.** The source is compiled as a single expression first. If that fails, it's compiled as a function body, which must `return` its value; a body that reaches its end without `return` gives `undefined`.

```js
vh.outputs[1].source == 3
```
```js
const names = { 1: "Stage", 2: "Lobby" };
return names[vh.outputs[1].source] ?? "Unknown";
```

**Scope.**

| Name | Contents |
|---|---|
| `trigger` | The trigger payload (§8.2) |
| `local` | Run-local variables set by `set` (§8.3) |
| `steps.<id>` | `{status, result?, error?, startedAt, durationMs}` for steps that have finished in this run. Steps that haven't run, or are still running, read as `undefined` |
| `<alias>` | A connection's state |
| `vars`, `sys`, `flow` | User variables, core state, flow status ([state spec](state-and-event-log.md) §1.1) |
| the standard library | `mmss`, `hhmmss`, `clamp`, `round`, `pad`, `lerp`, `rgb`, `hsl`, `mix` ([§11.2](#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)) |

- **Scope outside a run.** In filters, conditions and `state` triggers there's no run, so `local` and `steps` are empty.
- **Missing values.** Paths that don't exist read as `undefined` and never throw.
- **Truthiness** is JavaScript's, wherever an expression decides something: filters, conditions, `when`, `until`.

**Sandbox** ([§11.2](#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)).
- Everything in scope is read-only, and writing to it throws.
- There is no host API.
- No state carries over between evaluations.
- `Date.now()`, `new Date()` without arguments and `Math.random()` throw. Time is read from `sys.time.now`.
- A returned promise is an error.

**Values out.** The result is converted to a `Value` (state spec §1.2):
- Integral numbers within ±2⁵³ become ints, and other finite numbers floats.
- `undefined` means "no value": an absent parameter, or a removed local or variable.
- `NaN`, infinities, functions and symbols are errors.

**Dependencies.** Each evaluation records the state paths it read; that's its read set.
- `state` triggers and `waitUntil` re-evaluate when a path in their read set changes, once per scheduler turn, after patches are coalesced.
- The read set is replaced after each evaluation, so a dynamic read (`vh.outputs[vars.n].source`) follows `vars.n`.

**Limits and failures.** Each evaluation gets 50 ms and 16 MB; both are core settings. A failure is `{code, message, at?}`:

| Code | Meaning |
|---|---|
| `exprSyntax` | The source doesn't compile. Normally caught on save |
| `exprError` | It threw, including writes, banned calls, a returned promise and unconvertible results |
| `exprTimeout` | It ran past the time limit |
| `exprMemory` | It ran past the memory limit |

- In a step, a failure fails the step with that code.
- In a filter, condition or `state` trigger, it counts as falsy or `undefined` (§8.2), and is logged, rate-limited.
- `at` is `{line, column}` within the source, when known.

---

## 9. HTTP API

JSON throughout. Validation failures are `422 {errors, warnings}` (§8.4). Successful saves return the flow and any warnings.

| Method and path | Body | Returns |
|---|---|---|
| `GET /flows` | | `Flow[]` |
| `POST /flows` | a `Flow` without `id` | `201 {flow, warnings}` |
| `GET /flows/{id}` | | `Flow`, or `404` |
| `PUT /flows/{id}` | a whole `Flow` | `200 {flow, warnings}`. Replaces it; active runs keep their copy |
| `DELETE /flows/{id}` | | `204`. Cancels its runs and deletes `flow.<slug>` |
| `POST /flows/{id}/run` | `{inputs?}`, or no body | `200 {matched, status: started \| queued \| blocked, run?, reason?}`. `409` if the flow is disabled or has no `api` trigger. `422 {error}` if the inputs don't fit the flow's (§8.2) |
| `GET /runs?flow=<id>` | | `Run[]`, newest first: active, queued, then recent (§8.5) |
| `GET /runs/{run}` | | `Run`, or `404` once it has aged out |
| `POST /runs/{run}/cancel` | | `202 {run, status}`. `409` if it has already finished |
| `POST /expressions/evaluate` **(M4)** | `{source, trigger?}` | `200 {value?, error?, reads}`. See below |

```ts
interface Run {
  run: string;
  flow: string;                 // flow ID
  status: "queued" | "running" | "ok" | "failed" | "cancelled";
  matched: number;              // seq of trigger.matched
  started?: number;             // seq of run.started
  finished?: number;            // seq of run.finished
  startedAt?: number;           // ms since the epoch
  durationMs?: number;
  position?: string[];          // while running
  error?: { code: string; message: string };
  outputs?: { [name: string]: Value };   // what a `stop` step handed back
}
```

- `matched`, `started` and `finished` are journal sequence numbers, so the UI can open `why` on any of them.
- **(M4) `POST /expressions/evaluate`** is the editor's live preview.
  - **What it reads.** It evaluates `source` against live state, under the same sandbox and limits as a flow (§8.7). `trigger` is an optional sample payload, for previewing filters; `local` and `steps` are empty.
  - **Response.**
    - `value` is the result. It's absent when the result is `undefined`.
    - `error` is `{code, message, at?}`.
    - `reads` lists the text paths the evaluation read, so the editor can show what an expression depends on.
  - **No side effects.** It never writes, never runs actions and never journals.
- **No new WebSocket messages.** The UI subscribes to `flow` (or `flow.<slug>`) for status and reads `trigger.matched` and `run.*` entries from the journal tail ([state spec](state-and-event-log.md) §1.6).

---

## 10. Open questions

1. Should `local` names be declared up front (better types, clearer checkpoints), or is inferring them from `set` steps enough?
2. Should `queue` concurrency have a maximum queue length, and what happens when it's full? M3 uses 16 and blocks the trigger with `queueFull` (§8.5). Revisit with real shows.
3. Is a dedicated state-machine flow type needed for v1, or are loops, variables and separate flows enough? Collect real show flows before deciding.
4. Should called flows run inside the caller's run (shared cancellation, one timeline entry), or as linked runs? Proposal: a linked run, with cancellation passed down.
5. Step-level `retry` on blocks: allow it, or keep it on leaf steps only, to avoid re-running parts of a block that already took effect?

---

## 11. Decisions

Why flows and expressions are the way they are, and what was rejected. Keep these notes when the sections above change.

### 11.1 Flows are structured step trees, and control flow only
- **Decided:** a flow is stored as a tree of blocks (sequence, branch, loop, parallel, group with catch and finally) and leaf steps, each with a stable ID unique in its flow. No goto: retrying, early exit and loop control are steps or policies. The node-graph view draws the tree, with blocks as nested regions and edges meaning "then"; node positions are editor-only metadata the runtime ignores. Data inside a run moves through expressions (§3), not wires. Types are checked when editing (§4), and values stay dynamic at run time. Continuous dataflow (a label showing a timer, a fader driving a volume) is bindings and dataflow, not flows.
- **Why:** for control flow a free graph and a structured tree are equally expressive, and the tree is what the other requirements want:
  - a flat sequence *is* a Button-view flow, so moving down a view is a shape check, not graph analysis (§6)
  - a run's position is a path of step IDs, so a checkpoint is that plus the locals, and `resume` on failover stays simple
  - parallel blocks own their children, so "wait for all" and "first wins" are well defined, and the fork/join bug (a join waiting for a path that went elsewhere) can't happen
  - cancellation and error handling follow the nesting: a group's `catch` covers exactly its steps
  - the YAML export nests like code and diffs cleanly

  Free-graph tools are mostly dataflow tools (TouchDesigner, Max, Node-RED). Flows are the cue-list model of QLab, Companion action lists and Home Assistant scripts.
- **Costs:** state machines and jump-back logic need a loop, a variable, separate flows or a script (open question 3). The Flow view needs a nested-block editor with auto-layout of our own; Vue Flow can still render it.
- **Rejected:**
  - *A free graph of nodes and edges:* the cheapest editor, but view round-tripping, checkpoints, parallel joins and diffs get harder, with validation for unreachable nodes, dangling edges and cycles.
  - *A free graph limited to structured shapes:* "this connection isn't allowed" errors that are hard to explain, and a tree underneath anyway.
  - *Mixed control and data edges,* as in Unreal Blueprints: known for unreadable graphs, and duplicates expressions.

### 11.2 Expressions and scripts: one language, sandboxed by where code runs
- **Decided:** JavaScript/TypeScript everywhere, on QuickJS-ng in the core (`rquickjs`), with no difference in syntax between levels. **Code that computes is an expression and may run anywhere, any number of times. Code that acts is a script step and runs once, inside a run.**

  | | Expression | Script step |
  |---|---|---|
  | Used in | Action params, `wait.for`, filters, conditions, `when`, `waitUntil`, `branch`, `set` values, bindings, sources, the editor's preview | One `script` step |
  | Runs | Whenever the core decides: on state changes, on the standby, in preview | Once, at a known point in a run |
  | Syntax | One expression, or a function body that `return`s | Full JS/TS, `async/await` |
  | Can act | No | `action()`, `setVar()`, `setLocal()`, `emit()`, `wait()`, `waitUntil()`, `log` |
  | Time and randomness | None: read `sys.time.now` | `now()` and `random()`, journaled as `nondet` |

  Expressions are **pure by sandbox, not syntax** (§8.7): every name is a read-only proxy, no host API, no memory between evaluations, no `Date.now()`, `Math.random()` or async. Nothing in the core has file, network or process access: side effects go through `action()` and the output gate. Limits per evaluation: 50 ms (interrupt hook) and 16 MB by default, scripts with their step's `timeout`; the turn thread uses tighter ones (state spec §7.3). An expression that fails fails its step (`exprError`, `exprTimeout`, `exprMemory`); in a trigger, filter or condition it counts as false and is logged. **Dependencies are tracked at run time:** each evaluation records the paths it reads, and consumers re-subscribe to that read set, once per turn. User variables are the `vars` root, since `var` is a keyword. `oxc` stays out of the core until script steps need TypeScript stripped; until then QuickJS reports syntax errors, and unknown step IDs are editor warnings that read as `undefined` at run time.
- **Planned with script steps:** a small standard library (`mmss`, `hhmmss`, `clamp`, `round`, `pad`, `lerp`, `rgb`, `hsl`, `mix`); generated TypeScript definitions from the live state tree and manifests, with CodeMirror 6 for inline fields and Monaco for scripts; missing or offline values read as `undefined`, with fallbacks on bindings.
- **Why:** one language that carries from a label to a script to a plugin, and one engine, so code behaves the same wherever it runs. Scripts run next to the state: no IPC, cancellation at any instruction, compatible with standby shadowing and replay. Syntax was a poor proxy for side effects: a lookup table or a small loop changes nothing, while what can actually act is whatever the host hands in, which is ours to control. Static path extraction can't follow dynamic reads (`vh.outputs[vars.n].source`), and runtime read sets can. QuickJS is 20–50× slower than V8 on heavy computation, which doesn't matter for glue logic; heavy work belongs in a plugin.
- **Rejected:**
  - *Expression-only syntax checked by `oxc`:* forbids harmless code, adds a large dependency, and enforces purity in the wrong place. Checking only in the editor would let flows from the API or files carry side effects.
  - *The full host API in expressions:* their evaluation count is an implementation detail, so a repaint or a keystroke in the preview could fire an action with no run to belong to.
  - *No sandbox for scripts, since editors are trusted:* direct I/O bypasses the gate, so a standby would send commands twice, replay would touch devices, and a `while (true)` would freeze the core mid-show.
  - *Running scripts in the Deno host:* IPC, a crash domain shared with plugins, and harder failover.
  - *Other languages:* CEL is safe but unfamiliar, with strict number types (the fallback if JS proves too costly). Luau was strongest (speed, sandboxing, familiar from lighting consoles), but it's a second language next to TS plugins, fits JSON-shaped state badly (1-based arrays, `nil` holes) and has weaker browser tooling. Rhai, Rune, Lua, Starlark, JSONata or a custom syntax had the same problem or worse. A custom JS-subset evaluator would drift from real JS.

### 11.3 Inputs, outputs and calls
- **Decided:** a flow declares `inputs` as `ParamDef`s (the shape templates use for `params`, composition §2.1) and `outputs` as schema nodes. A flow with a `call` trigger can be started by a `call` step, which waits for its outputs unless `wait: false`, and a `stop` step hands them back. Inputs are bound and checked when the trigger fires, so a bad call fails the caller's step before the callee starts. Only `call` and `api` give inputs; other triggers bind defaults, so a required input with no default rules them out on save. The callee keeps its own concurrency, a waiting caller's cancel or timeout cancels the callee, and the loop guard (§8.6) stops recursion.
- **Why:** composite actions and owned flows (composition §6.2) need a flow that takes values and returns some, run per call with its own journal identity. Checking at the call keeps the failure where the mistake is. Running the callee under its own concurrency means a "recall camera" flow set to `queue` stays serialised however many callers it has. A waited call is part of the caller, so it goes when the caller goes.
- **Rejected:**
  - *Callers passing anything, checked by the callee's steps:* a typo in an input name would only show up as `undefined` somewhere inside the callee.
  - *Any flow callable, with no `call` trigger:* there'd be no way to see, or stop, which flows are meant to be called.
  - *Calls inlined into the caller's run:* the callee's concurrency and `flow.<slug>` status would mean nothing, and a flow run from a key and from a call would journal differently.
  - *A depth limit for recursion:* the loop guard already blocks a flow from re-entering its own chain, and there's no use for a flow calling itself.
