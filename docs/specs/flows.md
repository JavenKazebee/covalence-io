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

## 8. The M3 subset

[Milestone 3](../milestones/m3-first-flows.md) implements the Button-view shape (§6) without expressions. This section is what the core accepts until QuickJS arrives. **Anything outside it is rejected on save with an error, never stored and ignored.** Types live in `covalence_core::flow` ([ADR 0020](../adr/0020-project-entity-types-in-the-core.md)).

### 8.1 Flow fields

| Field | M3 |
|---|---|
| `id` | Assigned by the server on create (ULID). Ignored in a create body. Must match the URL on update |
| `name` | Required, non-empty |
| `slug` | Optional on create: derived from `name` (camelCase, ASCII, deduplicated with a number). Must match `[a-z][A-Za-z0-9_]*` and be unique |
| `enabled` | Required |
| `triggers` | `api` and `event` only (§8.2). May be empty, with a warning: the flow can't start |
| `conditions`, `inputs`, `outputs` | Must be absent |
| `concurrency` | Required. All four modes |
| `failover` | Optional; only `abort` (the default) |
| `body` | `action` and `wait` steps only (§8.3); no blocks |
| `editor` | Stored as-is, never read by the runtime |

### 8.2 Triggers

| Kind | Fields | Notes |
|---|---|---|
| `api` | none | Fired by `POST /flows/{id}/run` (§9). A flow without one can't be run from the API or the UI's Run button |
| `event` | `source: {conn}`, `event` | `event` must be in the plugin manifest's `events`. No `filter`, no `{plugin}` or `{interface}` sources |

`trigger.matched` names the trigger by its index in `triggers`.

### 8.3 Steps

Common fields: `id`, `kind`, `name?`, `enabled?`, `timeout?`, `onError?`. `retry` is rejected.

| Kind | Fields | Notes |
|---|---|---|
| `action` | `target: {conn}`, `action`, `params` | A fixed target with no `key`. `params` are literal values, checked against the action's params schema. A value shaped `{expr: ...}` is rejected as "expressions aren't supported yet" |
| `wait` | `for: Duration` | A duration string only |

- **Durations** match `^(\d+h)?(\d+m)?(\d+s)?(\d+ms)?$`, are not empty, and are greater than zero: `500ms`, `2s`, `1m30s`.
- **An action step** fails if the action returns an error (with that error), if the output gate suppresses it (code `suppressed`, with the reason), or if its connection isn't running (code `unavailable`). Its `result` is the action's result.
- **A `timeout`** fails the step with code `timeout`. A late `action.result` is still journaled but no longer affects the run.
- **`enabled: false`** journals the step as `skipped`.

### 8.4 Validation

Errors block the save; warnings don't. There are no drafts yet.

- **Errors:** anything outside §8.1–8.3, a duplicate or malformed step ID, a duplicate slug, an unknown connection, an action or event the connection's plugin doesn't declare, parameters that fail the action's schema, a bad duration.
- **Warnings:** no triggers; an `event` trigger on a connection the flow also acts on (see §8.6); a connection whose plugin hasn't loaded yet, so its actions and events couldn't be checked.
- Each problem is `{path, code, message}`, where `path` is a segment array into the flow, e.g. `["body", 1, "params", "input"]`.
- Stored flows aren't revalidated at startup. If a connection disappears, its steps fail at run time with `unavailable`.

### 8.5 Runs

- **A run** has a ULID, a status (`queued`, `running`, `ok`, `failed`, `cancelled`), and a copy of the flow taken when it started. Editing or disabling a flow doesn't touch runs already active. Deleting it cancels them.
- **Concurrency.**
  - `queue`: a queued trigger gets its run ID straight away but journals nothing more until it starts. The queue holds at most 16 runs. Past that, the trigger is blocked with reason `queueFull` (§10 question 2).
  - `restart`: cancels the active run and starts the new one once the old run has journaled `run.finished`.
- **Cancellation** happens at step boundaries and during a wait. An action already in flight is waited for: its step ends `done` or `failed`, then the run ends `cancelled`. Cancelling a queued run removes it and journals `run.finished cancelled`.
- **Journal entries for a run** carry `run` in the envelope and origin `{flow, run, step?}` (`step` only on step-level entries). Causes:

  | Entry | `cause` → |
  |---|---|
  | `trigger.matched` | the `event` entry; none for `api` (origin is the user) |
  | `run.started` | `trigger.matched` |
  | `run.step` | `run.started` |
  | `action.requested` | that step's `run.step started` |
  | `run.finished` | `run.started` (or `trigger.matched` for a queued run that never started) |

- `run.started` data is `{flow, run, mode, inputs}`, with `mode` the flow's concurrency mode and `inputs` `{}`.
- `run.step` `detail` is `{position, error?}`. In M3 the position is `[stepId]`.
- `trigger.matched` gains a `reason` when `blocked`: `ignore` (concurrency), `queueFull`, `loop` (§8.6), or `condition` once conditions exist.
- **State:** `flow.<slug>.running` and `flow.<slug>.lastRun.{status, startedAt, durationMs, error?}` (§5.4). `startedAt` is milliseconds since the epoch, like a leaf's `ts`. Deleting a flow deletes its subtree.
- **The core keeps** every active and queued run plus the last 100 finished runs in memory, for `GET /runs`. The journal is the record; the list is a convenience and is empty after a restart.

### 8.6 Loop guard

Without filters, a flow that listens to an event on a connection and also acts on that connection re-triggers itself forever. Two flows can do the same to each other.

- **At run time, a trigger is blocked with reason `loop`** when the entry that fired it descends, through `cause`, from a run of the same flow. That catches self-loops and loops through other flows. The check follows the chain only back to its first `run.started` of this flow, so it stays short.
- **On save,** an `event` trigger on a connection the flow's own steps act on is a warning (§8.4).
- **Known gap:** a live device's echo often arrives without a `cause` ([M1 known gaps](../milestones/m1-walking-skeleton.md#known-gaps-carried-forward)), so the guard can't see live loops. Event filters, which come with expressions, are the real fix.

---

## 9. HTTP API (M3)

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
- **No new WebSocket messages.** The UI subscribes to `flow` (or `flow.<slug>`) for status and reads `trigger.matched` and `run.*` entries from the journal tail ([state spec](state-and-event-log.md) §1.6).

---

## 10. Open questions

1. Should `local` names be declared up front (better types, clearer checkpoints), or is inferring them from `set` steps enough?
2. Should `queue` concurrency have a maximum queue length, and what happens when it's full? M3 uses 16 and blocks the trigger with `queueFull` (§8.5). Revisit with real shows.
3. Is a dedicated state-machine flow type needed for v1, or are loops, variables and separate flows enough? Collect real show flows before deciding.
4. Should called flows run inside the caller's run (shared cancellation, one timeline entry), or as linked runs? Proposal: a linked run, with cancellation passed down.
5. Step-level `retry` on blocks: allow it, or keep it on leaf steps only, to avoid re-running parts of a block that already took effect?
