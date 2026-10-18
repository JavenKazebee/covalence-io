# Spec: Flows

- **Status:** Draft v0.1
- **Related:** [ADR 0018](../adr/0018-flows-as-step-trees.md), [ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md), [ADR 0004](../adr/0004-javascript-expressions-and-scripts.md), [ADR 0008](../adr/0008-redundancy-and-failover.md), [ADR 0014](../adr/0014-time-sources-and-scheduling.md), [state and event log](state-and-event-log.md), [bindings](bindings.md)

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
| `set` | `path: "local.<name>" \| "var.<name>"`, `value: Expr` | none |
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
| state roots | `<alias>`, `var`, `sys`, `surface`, `page`, `flow` ([state spec §1.1](state-and-event-log.md)) | `var` via `set` and scripts |
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
      expr: var.selectedCam
      constraint: { interface: camera.ptz@1 }
    action: recallPreset
    params: { preset: { expr: page.params.shot } }
    timeout: 2s
    retry: { attempts: 2, backoff: 200ms }
  - id: settle
    kind: waitUntil
    until: "!target(var.selectedCam).moving"
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

## 8. Open questions

1. Should `local` names be declared up front (better types, clearer checkpoints), or is inferring them from `set` steps enough?
2. Should `queue` concurrency have a maximum queue length, and what happens when it's full?
3. Is a dedicated state-machine flow type needed for v1, or are loops, variables and separate flows enough? Collect real show flows before deciding.
4. Should called flows run inside the caller's run (shared cancellation, one timeline entry), or as linked runs? Proposal: a linked run, with cancellation passed down.
5. Step-level `retry` on blocks: allow it, or keep it on leaf steps only, to avoid re-running parts of a block that already took effect?
