# Spec: Flows

- **Status:** Draft v0.1
- **Related:** [ADR 0018](../adr/0018-flows-as-step-trees.md), [ADR 0020](../adr/0020-project-entity-types-in-the-core.md), [ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md), [ADR 0004](../adr/0004-javascript-expressions-and-scripts.md), [ADR 0008](../adr/0008-redundancy-and-failover.md), [ADR 0014](../adr/0014-time-sources-and-scheduling.md), [state and event log](state-and-event-log.md), [bindings](bindings.md)

A flow is the single unit of automation: triggers, optional conditions, and a body of steps. Button action lists, rules and node graphs are all flows. Flows are **control flow only**. Values move through expressions, not edges ([ADR 0018](../adr/0018-flows-as-step-trees.md)).

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
  failover: "abort" | "resume" | "restart";       // ADR 0008

  inputs?: { [name: string]: StateSchemaNode };   // for call-flow, API and OSC triggers
  outputs?: { [name: string]: StateSchemaNode };  // returned by `stop` (§2.3) to a caller

  body: Step[];                       // a sequence
  editor?: EditorMeta;                // view-only metadata (§6); ignored by the runtime
}

type Expr = string;                   // JS expression source (ADR 0004)
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
| `gesture` | `control: ControlRef`, `gesture: press \| release \| double \| long \| hold \| turn` | `{surface, control, gesture, delta?}` |
| `event` | `source: Target \| TargetConstraint`, `event`, `filter?: Expr` | `{source, event, payload}`, with `payload` typed by the `EventDef` |
| `state` | `when: Expr`, `edge: rising \| falling \| change` | `{value, previous}` |
| `schedule` | per [ADR 0014](../adr/0014-time-sources-and-scheduling.md) | `{scheduled, fired}` |
| `call` | none; any flow may call this one | `{caller: {flow, run}}` |
| `api` / `osc` | `address?` for OSC | `{args}` |
| `startup` | none | `{reason: boot \| failover \| publish}` |

- An `event` trigger can listen to an interface instead of one connection. `source: {interface: "camera.ptz@1"}` fires for any camera, and `trigger.source` says which ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)).
- `state` triggers are edge-triggered. Their dependencies are tracked at run time, like bindings.

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
| `action` | `target: TargetSpec`, `action`, `params: {[k]: Value \| {expr: Expr}}` | The action's `result`, per its `ActionDef` |
| `wait` | `for: Duration \| Expr` | none |
| `waitUntil` | `until: Expr`, `timeout?` | `{value}`. Checked immediately (level, not edge), then on every change |
| `call` | `flow`, `inputs?`, `wait?: boolean` (default `true`) | The callee's `outputs`, or `{run}` if `wait` is `false` |
| `set` | `path: "local.<name>" \| "vars.<name>"`, `value: Expr` | none |
| `script` | `source` (JS/TS), `timeout?` | Whatever the script returns |
| `log` | `level`, `message: Expr` | none |
| `stop` | `status?: ok \| failed`, `outputs?`, `error?` | ends the run |
| `break` / `continue` | none | only inside a `loop` |

```ts
type TargetSpec =
  | { conn: string; key?: PathSegment }                    // fixed
  | { expr: Expr; constraint: TargetConstraint };          // dynamic (ADR 0019)

type TargetConstraint = { plugin: string } | { interface: string };   // "camera.ptz@1"
```

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
| `input` | The flow's inputs | no |
| `local` | Run-local variables. They last for the run and are included in checkpoints | via `set` and scripts |
| `steps.<id>` | `{status, result?, error?, startedAt, durationMs}` for steps that have run | no |
| `<as>`, `index` | The current loop item and index, inside `forEach` | no |
| `error` | The error, inside a `catch` | no |
| state roots | `<alias>`, `vars`, `sys`, `surface`, `page`, `flow` ([state spec §1.1](state-and-event-log.md)) | `vars` via `set` and scripts |
| `target(t)` | A target's state ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) | no |

- **Steps in a branch that didn't run are absent** from `steps`. Reading them gives `undefined`, like any missing value ([ADR 0004](../adr/0004-javascript-expressions-and-scripts.md)).
- **Inside a `parallel` block,** a branch can only see its own steps and those that finished before the block started. Other branches' steps become visible after the join.
- **Scripts** get the same scopes as globals, plus the host API from ADR 0004. `setLocal(name, value)` joins `setVar`.

---

## 4. Typing

Types are checked **when editing**. Values stay dynamic at runtime.

- The editor generates TypeScript definitions for each flow from:
  - the trigger kind and the `EventDef` payload schema
  - the flow's `inputs`
  - each action's `result` schema, for `steps.<id>.result`
  - `local` names, inferred from `set` steps. A `local` may also be declared with a schema in `editor.locals`
  - the state schema and plugin manifests, as in ADR 0004
  - interface schemas, for dynamic targets
- Action `params` are checked against the action's schema, or against the interface's when the target is an interface.
- **Errors block publishing. Warnings don't.** Drafts can be saved with errors ([ADR 0009](../adr/0009-multi-user-editing.md)).
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
- Actions log `action.requested` with `cause` → the `run.step` entry. A dynamic target also records the resolved `conn`, `key` and `via` ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)).
- `retry` attempts each journal a `run.step` entry, with `attempt` in `detail`.

### 5.4 Flow status in state
`flow.<slug>.running` (count of active runs), `flow.<slug>.lastRun.{status, startedAt, durationMs, error?}`.

---

## 6. Views

The three views are three ways of showing the same tree.

| View | Shows a flow when | Notes |
|---|---|---|
| **Button** | One `gesture` trigger, no conditions, and a body of only `action`, `wait`, `set` and `log` steps | Edits add steps to the flat list |
| **Rule** | One trigger, and a body that is either the Button shape or a single `branch` whose arms have that shape | Conditions show as the "if" card |
| **Flow** | Any flow | Blocks are nested regions. Edges show order only |

- "Open as flow" is always possible. Going back down is offered when the tree matches the simpler shape.
- `editor` holds view-only data: node positions, collapsed blocks, colours and comments. It's exported, so a layout survives a round trip, but it never affects a run and its changes are left out of the diff summary.
- Without saved positions, the Flow view lays the tree out automatically from top to bottom.

---

## 7. Text export

Flows export as YAML ([ADR 0006](../adr/0006-project-storage-and-versioning.md)), one file per flow. Connections are stored as IDs and shown with their alias as a comment.

```yaml
id: 01JBZ8X4M2K9Q7N3T5V6W8Y0A1
name: Recall shot on selected camera
slug: recallShot
enabled: true
triggers:
  - kind: gesture
    control: { page: camControl, control: k8 }
    gesture: press
concurrency: restart
failover: abort
body:
  - id: recall
    kind: action
    target:
      expr: vars.selectedCam
      constraint: { interface: camera.ptz@1 }
    action: recallPreset
    params: { preset: { expr: page.params.shot } }
    timeout: 2s
    retry: { attempts: 2, backoff: 200ms }
  - id: settle
    kind: waitUntil
    until: "!target(vars.selectedCam).moving"
    timeout: 5s
    onError: continue
  - id: cut
    kind: action
    target: { conn: 01JBZ7P0D3F5G7H9J1K3L5M7N9 }   # atem
    action: cut
    params: {}
```

Keys appear in a fixed order. Fields that are always expressions (`when`, `until`, `while`, `filter`, `value`) hold plain strings. Action parameters are literal values unless wrapped in `{expr: ...}`, so a string parameter is never mistaken for code.

---

## 8. The implemented subset

This section is what the core accepts today.
- [Milestone 3](../milestones/m3-first-flows.md) implemented the Button-view shape (§6) without expressions.
- [Milestone 4](../milestones/m4-expressions.md) adds expressions (§8.7), `state` triggers, `event` filters, flow conditions, and the `branch`, `waitUntil` and `set` steps. Rules that M4 adds or changes are marked **(M4)**.
- [Milestone 5](../milestones/m5-surfaces-and-bindings.md) adds `gesture` triggers, marked **(M5)**. Planned, not yet implemented.

**Anything outside this section is rejected on save with an error, never stored and ignored.** Types live in `covalence_core::flow` ([ADR 0020](../adr/0020-project-entity-types-in-the-core.md)).

### 8.1 Flow fields

| Field | Accepted |
|---|---|
| `id` | Assigned by the server on create (ULID). Ignored in a create body. Must match the URL on update |
| `name` | Required, non-empty |
| `slug` | Optional on create: derived from `name` (camelCase, ASCII, deduplicated with a number). Must match `[a-z][A-Za-z0-9_]*` and be unique |
| `enabled` | Required |
| `triggers` | `api`, `event`, **(M4)** `state` and **(M5)** `gesture` (§8.2). May be empty, with a warning: the flow can't start |
| `conditions` | **(M4)** Optional list of expressions. All must be truthy when a trigger matches, or no run starts (§8.2) |
| `inputs`, `outputs` | Must be absent |
| `concurrency` | Required. All four modes |
| `failover` | Optional; only `abort` (the default) |
| `body` | `action`, `wait` and **(M4)** `waitUntil`, `set` and `branch` steps (§8.3). `branch` is the only block |
| `editor` | Stored as-is, never read by the runtime |

### 8.2 Triggers

| Kind | Fields | `trigger` in expressions | Notes |
|---|---|---|---|
| `api` | none | `{args: {}}` | Fired by `POST /flows/{id}/run` (§9). A flow without one can't be run from the API or the UI's Run button |
| `event` | `source: {conn}`, `event`, **(M4)** `filter?: Expr` | `{source, event, payload}` | `event` must be in the plugin manifest's `events`. No `{plugin}` or `{interface}` sources |
| `state` **(M4)** | `when: Expr`, `edge: rising \| falling \| change` | `{value, previous}` | See below |
| `gesture` **(M5)** | `control: {page, control}`, `gesture: press \| release \| double \| long \| hold` | `{surface, page, control, gesture, delta?}` | Matches on any surface showing the page. Gestures and their timing are in the [surfaces spec §3](surfaces.md). `trigger.matched` has `cause` → the `input.gesture` entry |

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

### 8.3 Steps

Common fields: `id`, `kind`, `name?`, `enabled?`, `timeout?`, `onError?`. `retry` is rejected. Step IDs are unique across the whole flow, including steps nested in `branch` arms.

| Kind | Fields | Result (`steps.<id>.result`) | Notes |
|---|---|---|---|
| `action` | `target: {conn}`, `action`, `params` | The action's result | A fixed target with no `key`. **(M4)** Each top-level parameter is a literal value or `{expr: Expr}`. See below |
| `wait` | `for: Duration` or **(M4)** `{expr: Expr}` | none | The expression must give a duration string or a number of milliseconds ≥ 0. Anything else fails the step with `invalidDuration` |
| `waitUntil` **(M4)** | `until: Expr`, `timeout?` | `{value}` | Evaluated when the step starts, and done straight away if truthy (level, not edge). Otherwise re-evaluated whenever its read set changes, until truthy. `timeout` fails it with `timeout` |
| `set` **(M4)** | `path: "local.<name>" \| "vars.<name>"`, `value: Expr` | none | `<name>` is one segment, `[A-Za-z_][A-Za-z0-9_]*`. See below |
| `branch` **(M4)** | `arms: {when: Expr, steps: Step[]}[]`, `else?: Step[]` | `{status}` | See below |

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
- **A `timeout`** fails the step with code `timeout`. A late `action.result` is still journaled but no longer affects the run.
- **`enabled: false`** journals the step as `skipped`. A skipped `branch` skips its arms without journaling them.

### 8.4 Validation

Errors block the save; warnings don't. There are no drafts yet.

- **Errors:**
  - anything outside §8.1–8.3
  - a duplicate or malformed step ID
  - a duplicate slug
  - an unknown connection
  - an action or event the connection's plugin doesn't declare
  - literal parameters that fail the action's schema
  - a bad literal duration
  - **(M4)** an expression that doesn't compile, a malformed `set` path, a `branch` with no arms
  - **(M5)** a `gesture` trigger on a page that doesn't exist, or a control outside its layout (`unknownControl`)
- **Warnings:**
  - no triggers
  - an `event` trigger on a connection the flow also acts on (see §8.6)
  - a connection whose plugin hasn't loaded yet, so its actions and events couldn't be checked
  - **(M4)** a `waitUntil` without a `timeout`
- **(M4) What the core doesn't check.**
  - **Parameter schemas, for any parameter that is an expression.** Its value is checked at run time, along with the rest of `params`.
  - **References to steps** (`steps.route9`). The editor warns about those, and at run time they read as `undefined` ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)).
- **Problem shape.** Each problem is `{path, code, message, at?}`.
  - `path` is a segment array into the flow, e.g. `["body", 1, "params", "input"]`.
  - **(M4)** For an expression that doesn't compile, the code is `exprSyntax` and `at` is `{line, column}` within the expression's source.
- Stored flows aren't revalidated at startup. If a connection disappears, its steps fail at run time with `unavailable`.

### 8.5 Runs

- **A run** has a ULID, a status (`queued`, `running`, `ok`, `failed`, `cancelled`), and a copy of the flow taken when it started. Editing or disabling a flow doesn't touch runs already active. Deleting it cancels them.
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
  | `run.started` | `trigger.matched` |
  | `run.step` | `run.started` |
  | `action.requested` | that step's `run.step started` |
  | `state.patch` **(M4)** | from a `set` step, that step's `run.step started` |
  | `run.finished` | `run.started` (or `trigger.matched` for a queued run that never started) |

- `run.started` data is `{flow, run, mode, inputs}`, with `mode` the flow's concurrency mode and `inputs` `{}`.
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

The language and sandbox are set by [ADR 0004](../adr/0004-javascript-expressions-and-scripts.md) and [ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md).

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
| the standard library | `mmss`, `hhmmss`, `clamp`, `round`, `pad`, `lerp`, `rgb`, `hsl`, `mix` ([ADR 0004](../adr/0004-javascript-expressions-and-scripts.md)) |

- **Scope outside a run.** In filters, conditions and `state` triggers there's no run, so `local` and `steps` are empty.
- **Missing values.** Paths that don't exist read as `undefined` and never throw.
- **Truthiness** is JavaScript's, wherever an expression decides something: filters, conditions, `when`, `until`.

**Sandbox** ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)).
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
| `POST /flows/{id}/run` | `{}` | `200 {matched, status: started \| queued \| blocked, run?, reason?}`. `409` if the flow is disabled or has no `api` trigger |
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
