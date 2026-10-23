# Spec: State model and event log

- **Status:** Draft v0.2
- **Related:** [ADR 0007](../adr/0007-event-sourced-core.md), [ADR 0008](../adr/0008-redundancy-and-failover.md), [ADR 0009](../adr/0009-multi-user-editing.md), [ADR 0027](../adr/0027-events-addressed-by-path.md), [ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md), [plugin protocol](plugin-protocol.md), [dataflow](dataflow.md)
- **Markers:** **(M6)**, **(M8)** and **(M9)** name the milestone that implements a rule, as in the [dataflow spec](dataflow.md).

Redundancy, multi-user editing, undo, history, the timeline and replay are all built on this spec. Get it right before writing engine code.

---

## 1. The state tree

All live values in the system share one namespace.

### 1.1 Paths

```
<root>.<segment>.<segment>…
```

| Root | Owner | Example |
|---|---|---|
| `<alias>` | A connection, named by its user-chosen alias | `pp.timers.countdown.remaining` |
| `vars` | User variables: writable, or **(M8)** computed and accumulated cells ([dataflow §6](dataflow.md)) | `vars.serviceStarted` |
| `sys` | The core | `sys.time.now`, `sys.node.role`, `sys.timecode.frames` |
| `surface` | Surfaces, their navigation and their controls' input state | `surface.fohDeck.current.main`, `surface.fohDeck.controls.k0.pressed` |
| `layer` | Per-layer state: parameters and helper values. **(Now)** Replaces M5's `page` root | `layer.cameraA.controls.dial1.position` |
| `flow` | Flow and run status | `flow.preService.running`, `flow.preService.lastRun.status` |

- **Segments** match `[A-Za-z_][A-Za-z0-9_]*`. Collection keys that aren't valid identifiers use bracket syntax: `vh.outputs["SDI 4"].source`.
- **Aliases** are unique per project, must not collide with the reserved roots, and are renameable. **(M6)** `core` is reserved too: it's the alias of the core's own connection ([core actions](core-actions.md)), whose state is the reserved roots.
- **(M6) Events are addressed by path:** an event happens at a node and has a name, written `<path>:<name>` (`pp:timerEnded`, `surface.deck.controls.k3:press`, `flow.preService:finished`). See [dataflow §3](dataflow.md).
- **User variables are `vars`, not `var`,** because `var` is a JavaScript keyword and couldn't be read in an expression ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)).
- **On the wire and in storage, paths are segment arrays** (`["outputs", "SDI 4", "source"]`), per [ADR 0017](../adr/0017-wire-paths-as-segment-arrays.md). The dotted text form is for expressions, the UI and logs.
- **The stored form uses IDs, not aliases.** The project stores references as `{conn: <connectionId>, path: [...]}`, and the alias is shown only when rendering text. Renaming an alias therefore never breaks a reference.

### 1.2 Values

```ts
type Value =
  | null | boolean
  | { int: bigint }        // i64 on the wire; plain number in JS when within safe range
  | number                 // f64
  | string
  | Value[]
  | { [key: string]: Value };
```

The **state schema** (declared by the plugin manifest, or by the user for variables) adds meaning to the plain types:
- `unit`: `ms`, `s`, `dB`, `%`, `Hz`, `K`, `fps`
- `kind`: `duration`, `timestamp`, `color`, `enum` (with labels), `percent`, `id`, `target` (a connection or item, with an optional `interface` constraint; proposed in [ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md))
- `range`, `step`
- `readOnly`
- `description`

The UI uses these for formatting, input widgets and autocomplete. Expressions see plain JS values.

### 1.3 Value metadata

Each leaf value carries:

| Field | Meaning |
|---|---|
| `ts` | Wall-clock time of the last change |
| `seq` | Journal sequence number of the patch that set it |
| `quality` | `good` · `stale` (source disconnected; the last value is kept) · `unknown` (never received) |
| `source` | Connection ID, `core` or `user` |

When a connection drops, its subtree is marked **`stale` rather than deleted**. The UI greys it out, and expressions can test it with `stale(pp.timers.countdown.remaining)`. Reading a path that doesn't exist returns `undefined`.

### 1.4 Dynamic collections

Devices have collections that change at runtime: ProPresenter timers, Videohub inputs and outputs, RED cameras on a network. The schema declares them as **collections** with a key type and an item schema:

```yaml
timers:
  collection: { key: id, label: name }
  item:
    name: { type: string }
    remaining: { type: number, unit: s, kind: duration }
    state: { type: string, kind: enum, values: [stopped, running, overrun] }
```

Action parameters can refer to a collection for their choices (`choices: { from: timers }`), so dropdowns fill with live values.

### 1.5 Change propagation

1. Patches arrive from hosts, drivers, scripts or the core.
2. They're applied to the tree and journaled.
3. A **dirty set** of changed paths goes to the binding evaluator.
4. The evaluator re-runs only the bindings and state triggers whose dependency set contains a dirty path. Dependency sets are tracked at run time ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)). **(M8)** Cells are evaluated first, in dependency order, and their changes join the dirty set within the same turn ([dataflow §5.3](dataflow.md)).
5. The surface renderer redraws only the controls whose resolved look changed.
6. UI clients receive patches only for the paths they subscribe to.

Patches are coalesced per path within one scheduler turn: the latest value wins, but **events are never coalesced**.

**(Now) Control input** ([ADR 0024](../adr/0024-surfaces-as-two-way-devices.md), [surfaces spec §4.1](surfaces.md)) is the exception. Each key, touch and encoder input is applied in a scheduler turn of its own, so no edge is lost. Fader and other `absolute` input is coalesced like any other patch. Input state is **volatile**: it's reset on attach and detach, isn't restored after a restart, and starts released on a node that has just been promoted (§4).

### 1.6 UI subscriptions (`GET /ws`)

UI clients get live state over a WebSocket at `/ws`. Each frame is one JSON text message. Paths use the text form (§1.1).

**Client → server** (`op`):

| Message | Does |
|---|---|
| `{op: "subscribe", paths: ["vh.outputs"]}` | Adds path prefixes and answers with a `snapshot` of them. An alias that doesn't exist yet is fine. |
| `{op: "unsubscribe", paths: [...]}` | Removes prefixes exactly as they were subscribed. Answers `unsubscribed`. |
| `{op: "journal", on: true}` | Starts or stops the live journal tail. Answers `{type: "journal", on}`. Use `GET /journal` for the backlog and dedupe by `seq`. |

**Server → client** (`type`):

| Message | Meaning |
|---|---|
| `host` `{host: {phase, generation, pid}}` | Plugin host status. Sent first, then on every change. |
| `snapshot` `{paths, leaves}` | Every leaf under `paths`. Replace whatever the client holds under them. |
| `patch` `{set, deleted}` | Changed leaves under the subscriptions, coalesced for about one frame. `deleted` lists text paths. |
| `journal` `{entry}` | One journal entry (§2.1). Never coalesced. |
| `lagged` `{feed: "journal", missed}` | The tail fell behind and dropped entries. Refetch them over HTTP. |
| `error` `{message}` | A bad request. The session carries on. |

Leaves have the same shape as in `GET /state`: `{path, value, quality, ts, seq}`. When a client falls behind the core's change feed, it gets a fresh `snapshot` of all its subscriptions instead of a gap. At shutdown the server closes the socket.

---

## 2. Two logs

| | **Project op log** | **Runtime journal** |
|---|---|---|
| Records | Configuration changes | What happened while running |
| Volume | Low | High (thousands/s possible) |
| Retention | Durable, compacted into snapshots | A configurable ring buffer (default 24 h or 1 GB), plus optional archiving |
| Used for | Multi-user editing, undo, history, restore points, replication | Timeline, "why did this fire?", replay, replication (hot standby) |
| Stored in | SQLite `project_ops` | SQLite `journal` (WAL), plus an in-memory ring |

### 2.1 Shared envelope

```ts
interface Entry {
  seq: bigint;          // increases by 1 within its log; no gaps
  term: number;         // leadership term (ADR 0008); 1 for single-node setups
  ts: string;           // wall clock, RFC 3339 with microseconds (for display and correlation only)
  mono: bigint;         // monotonic ns since node start (for ordering durations)
  origin: Origin;       // who produced it
  cause?: bigint;       // seq of the entry that directly caused this one (same log)
  run?: string;         // flow run ID this entry belongs to
  kind: string;         // see the tables below
  data: unknown;        // kind-specific payload
}

type Origin =
  | { user: string; client: string }          // a UI or API user
  | { conn: string }                          // a connection (via its host)
  | { surface: string; orbital?: string }
  | { flow: string; run: string; step?: string }   // no step on run-level entries
  | { core: "scheduler" | "system" | "replication" | "dataflow" };   // (M8) dataflow: cells and sinks
```

Ordering is defined **only** by `seq` within a log. `ts` is for display.

### 2.2 Runtime journal kinds

| Kind | Data | Notes |
|---|---|---|
| `input.raw` | `{surface, control, kind: down\|up\|turn\|touch\|value, value?, deviceTs}` | **(M5)** `surface` is the entity ID. `deviceTs` is the host's clock: the browser's, or the Orbital's if remote. **(Now)** Replaced by a `state.patch` on the control's input state |
| `input.gesture` | `{surface, page?, control, gesture: press\|release\|double\|long\|hold\|turn, delta?}` | **(M5)** `page` is the page ID shown when the input arrived. `cause` → the `input.raw` entry. **(Now)** Replaced by `input.event` |
| `input.event` | `{surface, layer?, control, event, delta?, value?, echo?, item?}` | **(Now)** An event derived from a control's input state ([surfaces spec §4.2](surfaces.md)). `layer` is the layer that controlled the control, absent if none did. `cause` → the input's `state.patch`, or for a timed `long` or `hold`, the `down`'s. **(Later)** `item` is the control group item it showed. **(M6)** Folded into `event` |
| `event` | `{source, event, payload}` | From a plugin, the core or a script. From a plugin action: `cause` → the `action.requested` entry. **(M6)** `{path, event, payload?, conn?, key?, surface?, layer?, control?, item?, echo?}`: `path` is the node it happened at, in ID form ([dataflow §3](dataflow.md)); the other fields are context. Control input events, helper events, flow events and `sys:startup` are all `event` entries |
| `state.patch` | `{ops: [{path, value} \| {path, delete: true}], deviceTs?}` | Paths stored in ID form. From a plugin action: `cause` → the `action.requested` entry ([plugin protocol](plugin-protocol.md) §6 `call`). **(Now)** From control input: origin the surface, and `deviceTs` the host's clock |
| `state.quality` | `{subtree, quality}` | Marks a subtree stale or good |
| `trigger.matched` | `{flow, trigger, blocked?, reason?}` | `cause` → the event, input event or patch. `reason` says why it was blocked ([flow spec](flows.md) §8.5) |
| `run.started` | `{flow, run, mode, inputs}` | |
| `run.step` | `{run, step, status: started\|done\|failed\|skipped, detail?}` | |
| `run.checkpoint` | `{run, step, locals}` | For `resume` on failover |
| `run.finished` | `{run, status: ok\|failed\|cancelled\|aborted, error?}` | |
| `action.requested` | `{call, conn, key?, via?, action, params, deadline?}` | `call` is the idempotency key. `key` and `via` (interface) record a resolved dynamic target ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) |
| `action.dispatched` | `{call}` | Passed the output gate |
| `action.suppressed` | `{call, reason: standby\|simulate\|showLock\|permission\|staleTerm}` | |
| `action.result` | `{call, ok, result?, error?, durationMs}` | |
| `nondet` | `{run, kind: now\|random, value}` | Recorded so replay is exact |
| `conn.status` | `{conn, status, message?}` | |
| `node.role` | `{node, role: active\|standby, term}` | |
| `log` | `{level, message, fields}` | Plugin and script logs |

**"Why did this fire?"** means walking `cause` backwards. For example:

`action.result` → `action.requested` → `run.step` → `run.started` → `trigger.matched` → `input.event` (**(M6)** `event`) → `state.patch` (the input)

**(M8)** Cells add links to the chain without breaking it: a computed variable's `state.patch` has `cause` → the patch it read, so `why` passes through it to the input.

### 2.3 Project op log

Entities: `connection` (**(M9)** including composites, plugin `core.composite`), `surface`, `surface_group` (Later), `layer` (M5: `page`), `control_binding` (which also holds a control's helpers, replacing `value_mapping`), `control_group` (Later), `flow`, `variable` (**(M8)** declared, [dataflow §6](dataflow.md)), **(M9)** `component`, `placement` and `transform` ([composition](composition.md), [dataflow §4.4](dataflow.md)), `asset`, `user`, `role_assignment`, `setting`, `restore_point`.

```ts
interface ProjectOp extends Entry {
  kind: "entity.create" | "entity.update" | "entity.delete" | "entity.move"
      | "publish" | "restore" | "migrate";
  data: {
    entity: { type: EntityType; id: string };   // ULID
    baseRev?: number;                            // entity revision the client saw
    rev: number;                                 // revision after applying
    set?: { [fieldPath: string]: unknown };      // field-level sets (update/create)
    unset?: string[];
    inverse: { set?: {...}; unset?: string[] };  // for undo
    draft?: string;                              // draft ID, when not yet published
  };
}
```

**Conflict rule:**
1. If `baseRev` matches the current revision, apply the op.
2. Otherwise, apply only the fields that weren't changed since `baseRev`.
3. Overlapping fields: last write wins, and the client whose value was overwritten is notified.

**Undo** applies `inverse`, adjusted to the current state of the fields involved.

**Compaction:** periodically fold the ops into an entity snapshot. Keep ops after the newest restore point, plus a configurable history window.

---

## 3. The output gate

Every side effect becomes an `action.requested` entry and then passes the gate:

```
requested ─► role == active? ─► mode != simulate? ─► show lock / role permits? ─► term current? ─► dispatched
                 │no                 │no                   │no                         │no
                 └──────────────── suppressed (reason) ◄────┴───────────────────────────┘
```

Surface output (key images, LEDs) is gated by role only. A standby node doesn't drive surfaces unless it has been promoted.

---

## 4. Replication (redundancy levels 2–3)

- The standby subscribes to both logs from `lastSeq + 1`, and **applies** entries; it doesn't recompute. It never evaluates triggers while it's standby.
- **On promotion:**
  1. Increase `term` and journal `node.role`.
  2. For each run that has no `run.finished` entry, apply its flow's failover policy (`abort` | `resume` from the last `run.checkpoint` | `restart`).
  3. Re-arm triggers and schedules.
  4. Request fresh snapshots from all connections.
  5. **(Now)** Reset every surface's control input state to released. Surfaces reattach to the new active node, and nothing a standby replicated counts as held.
- **Fencing:** hosts and Orbitals reject `action.invoke` messages carrying a term lower than the highest term they've seen.

---

## 5. Persistence layout (SQLite)

| Table | Contents |
|---|---|
| `entities` | `(type, id, rev, body JSON)`: the materialised project |
| `project_ops` | The project op log |
| `snapshots` | Restore points: a compacted copy of `entities` plus metadata |
| `journal` | The runtime journal (time- and size-limited) |
| `state_cache` | The last known state tree, so the UI shows `stale` values right after a restart |
| `assets` | Content-addressed blobs (icons, images) |
| `meta` | `schemaVersion`, node ID, current term |

Secrets aren't stored here ([ADR 0006](../adr/0006-project-storage-and-versioning.md)).

---

## 6. Open questions

1. Should integers be a distinct type in expressions, or should we always use f64 in JS and keep `int` only as a schema hint? (Leaning towards the hint.)
2. Journal retention defaults on a Raspberry Pi with an SD card. How much should we write, and how do we limit wear?
3. Should `run.checkpoint` be automatic at every step, or opt-in per flow? Automatic is simpler; opt-in costs less.
4. Should state patches from a connection be journaled in full, or sampled for very high-rate values (meters, timecode)? Proposal: the schema flag `journal: sampled(100ms)`. **(M8)** Decided for cells and helpers by [ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md): `journal: {sampled: "100ms"}` applies every value to state, journals the latest per interval, and a final entry when it settles ([dataflow §5.1](dataflow.md)). Connections' schemas get the same flag when a plugin needs it.
5. How should drafts interact with multi-user editing? Is one draft shared per layer, or does each user have their own?
