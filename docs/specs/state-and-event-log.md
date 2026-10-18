# Spec: State model and event log

- **Status:** Draft v0.1
- **Related:** [ADR 0007](../adr/0007-event-sourced-core.md), [ADR 0008](../adr/0008-redundancy-and-failover.md), [ADR 0009](../adr/0009-multi-user-editing.md), [plugin protocol](plugin-protocol.md)

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
| `var` | User variables | `var.serviceStarted` |
| `sys` | The core | `sys.time.now`, `sys.node.role`, `sys.timecode.frames` |
| `surface` | Surfaces and controls | `surface.fohDeck.page`, `surface.fohDeck.connected` |
| `page` | Per-page state | `page.cameraA.selected` |
| `flow` | Flow and run status | `flow.preService.running`, `flow.preService.lastRun.status` |

- **Segments** match `[A-Za-z_][A-Za-z0-9_]*`. Collection keys that aren't valid identifiers use bracket syntax: `vh.outputs["SDI 4"].source`.
- **Aliases** are unique per project, must not collide with the reserved roots, and are renameable.
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
- `kind`: `duration`, `timestamp`, `color`, `enum` (with labels), `percent`, `id`
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
4. The evaluator re-runs only the bindings and state triggers whose dependency set contains a dirty path. Dependency sets come from the static analysis in [ADR 0004](../adr/0004-javascript-expressions-and-scripts.md); dynamic accesses are tracked at runtime.
5. The surface renderer redraws only the controls whose resolved look changed.
6. UI clients receive patches only for the paths they subscribe to.

Patches are coalesced per path within one scheduler turn: the latest value wins, but **events are never coalesced**.

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
  | { flow: string; run: string; step: string }
  | { core: "scheduler" | "system" | "replication" };
```

Ordering is defined **only** by `seq` within a log. `ts` is for display.

### 2.2 Runtime journal kinds

| Kind | Data | Notes |
|---|---|---|
| `input.raw` | `{surface, control, kind: down\|up\|turn\|touch\|value, value?, deviceTs?}` | `deviceTs` comes from the Orbital's clock, if remote |
| `input.gesture` | `{surface, control, gesture: press\|release\|double\|long\|hold\|turn, delta?}` | `cause` → the `input.raw` entry |
| `event` | `{source, event, payload}` | From a plugin, the core or a script |
| `state.patch` | `{ops: [{path, value} \| {path, delete: true}]}` | Paths stored in ID form |
| `state.quality` | `{subtree, quality}` | Marks a subtree stale or good |
| `trigger.matched` | `{flow, trigger}` | `cause` → the event, gesture or patch |
| `run.started` | `{flow, run, mode, inputs}` | |
| `run.step` | `{run, step, status: started\|done\|failed\|skipped, detail?}` | |
| `run.checkpoint` | `{run, step, locals}` | For `resume` on failover |
| `run.finished` | `{run, status: ok\|failed\|cancelled\|aborted, error?}` | |
| `action.requested` | `{call, conn, action, params, deadline?}` | `call` is the idempotency key |
| `action.dispatched` | `{call}` | Passed the output gate |
| `action.suppressed` | `{call, reason: standby\|simulate\|showLock\|permission\|staleTerm}` | |
| `action.result` | `{call, ok, result?, error?, durationMs}` | |
| `nondet` | `{run, kind: now\|random, value}` | Recorded so replay is exact |
| `conn.status` | `{conn, status, message?}` | |
| `node.role` | `{node, role: active\|standby, term}` | |
| `log` | `{level, message, fields}` | Plugin and script logs |

**"Why did this fire?"** means walking `cause` backwards. For example:

`action.result` → `action.requested` → `run.step` → `run.started` → `trigger.matched` → `input.gesture` → `input.raw`

### 2.3 Project op log

Entities: `connection`, `surface`, `page`, `control_binding`, `flow`, `variable`, `asset`, `user`, `role_assignment`, `setting`, `restore_point`.

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
4. Should state patches from a connection be journaled in full, or sampled for very high-rate values (meters, timecode)? Proposal: the schema flag `journal: sampled(100ms)`.
5. How should drafts interact with multi-user editing? Is one draft shared per page, or does each user have their own?
