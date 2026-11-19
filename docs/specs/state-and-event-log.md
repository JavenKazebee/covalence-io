# Spec: State model and event log

- **Status:** Draft v0.3
- **Related:** [plugin protocol](plugin-protocol.md), [dataflow](dataflow.md)
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
- **User variables are `vars`, not `var`,** because `var` is a JavaScript keyword and couldn't be read in an expression ([flows §11.2](flows.md#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)).
- **On the wire and in storage, paths are segment arrays** (`["outputs", "SDI 4", "source"]`), per [plugin protocol §13.3](plugin-protocol.md#133-paths-on-the-wire-are-segment-arrays). The dotted text form is for expressions, the UI and logs.
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
- `kind`: `duration`, `timestamp`, `color`, `enum` (with labels), `percent`, `id`, `target` (a connection, with an optional constraint: a plugin or a composite template; proposed in [plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed))
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
| `reducer` | **(M8)** A reducer cell's `{transform, state?, wakeAt?}` ([dataflow §5.2](dataflow.md)), so it can resume after a failover or a restart. Not visible to expressions |

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
4. The evaluator re-runs only the bindings and state triggers whose dependency set contains a dirty path. Dependency sets are tracked at run time ([flows §11.2](flows.md#112-expressions-and-scripts-one-language-sandboxed-by-where-code-runs)). **(M8)** Cells are evaluated first, in dependency order, and their changes join the dirty set within the same turn ([dataflow §5.3](dataflow.md)).
5. The surface renderer redraws only the controls whose resolved look changed.
6. UI clients receive patches only for the paths they subscribe to.

Patches are coalesced per path within one scheduler turn: the latest value wins, but **events are never coalesced**.

**(Now) Control input** ([surfaces §11.2](surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state), [surfaces spec §4.1](surfaces.md)) is the exception. Each key, touch and encoder input is applied in a scheduler turn of its own, so no edge is lost. Fader and other `absolute` input is coalesced like any other patch. Input state is **volatile**: it's reset on attach and detach, isn't restored after a restart, and starts released on a node that has just been promoted (§4).

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
| Retention | Durable, compacted into snapshots | In memory: everything, for a window (default 1 h). On disk: what can't be recomputed, as a ring buffer (default 24 h or 1 GB), plus optional archiving (§2.4) |
| Used for | Multi-user editing, undo, history, restore points, replication | Timeline, "why did this fire?", replay, replication (hot standby) |
| Stored in | SQLite `project_ops` | SQLite `journal` (WAL), plus an in-memory ring |

### 2.1 Shared envelope

```ts
interface Entry {
  seq: bigint;          // increases by 1 within its log; no gaps
  term: number;         // leadership term (§7.4); 1 for single-node setups
  ts: string;           // wall clock, RFC 3339 with microseconds (for display and correlation only)
  mono: bigint;         // monotonic ns since node start (for ordering durations)
  origin: Origin;       // who produced it
  cause?: bigint;       // seq of the entry that directly caused this one (same log)
  inputCause?: bigint;  // (M8) when `cause` is a derived entry: the nearest input entry among its causes (§2.4)
  run?: string;         // flow run ID this entry belongs to
  instance?: InstanceChain;   // (M9) the instances it was made inside, outermost first (composition §5)
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
| `event` | `{source, event, payload}` | From a plugin, the core or a script. From a plugin action: `cause` → the `action.requested` entry. **(M6)** `{path, event, payload?, conn?, surface?, layer?, control?, item?, echo?}`: `path` is the node it happened at, in ID form ([dataflow §3](dataflow.md)); the other fields are context. Control input events, helper events, flow events and `sys:startup` are all `event` entries |
| `state.patch` | `{ops: [{path, value} \| {path, delete: true}], deviceTs?}` | Paths stored in ID form. From a plugin action: `cause` → the `action.requested` entry ([plugin protocol](plugin-protocol.md) §6 `call`). **(Now)** From control input: origin the surface, and `deviceTs` the host's clock |
| `state.quality` | `{subtree, quality}` | Marks a subtree stale or good |
| `trigger.matched` | `{flow, trigger, blocked?, reason?}` | `cause` → the event, input event or patch. `reason` says why it was blocked ([flow spec](flows.md) §8.5) |
| `run.started` | `{flow, run, mode, inputs, params?}` | **(M9)** `params`: the owner instance's bound values when the run started, for a run inside an instance ([composition §5](composition.md#5-identity-validation-and-detach)) |
| `run.step` | `{run, step, status: started\|done\|failed\|skipped, detail?}` | |
| `run.checkpoint` | `{run, step, locals}` | For `resume` on failover |
| `run.finished` | `{run, status: ok\|failed\|cancelled\|aborted, error?}` | |
| `action.requested` | `{call, conn, action, params, deadline?}` | `call` is the idempotency key. `conn` is the resolved connection of a dynamic target ([plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed)) |
| `action.dispatched` | `{call}` | Passed the output gate |
| `action.suppressed` | `{call, reason: standby\|simulate\|showLock\|permission\|staleTerm}` | |
| `action.result` | `{call, ok, result?, error?, durationMs}` | |
| `nondet` | `{run, kind: now\|random, value}` | Recorded so replay is exact |
| `instance.added`, `instance.removed` | `{instance: InstanceChain}` | **(M9)** A per-item instance created or ended with its list ([composition §4](composition.md)). `cause` → the patch or domain change that changed the list |
| `project.applied` | `{op, entity: {type, id}, rev}` | **(M8)** A project op (§2.3) took effect in the running core, in this turn. `op` is its `seq` in the project op log, which a journal `cause` can't name directly. A batch, or an undo of one, that touches several entities is one entry, with `op` the `batch` op (§7.6). What the edit changed in state (a cell reset, a stateless cell's new value, a layer re-resolved) has `cause` → this entry ([dataflow §5.4](dataflow.md)) |
| `conn.status` | `{conn, status, message?}` | |
| `node.role` | `{node, role: active\|standby, term}` | |
| `log` | `{level, message, fields}` | Plugin and script logs |

**"Why did this fire?"** means walking `cause` backwards. For example:

`action.result` → `action.requested` → `run.step` → `run.started` → `trigger.matched` → `input.event` (**(M6)** `event`) → `state.patch` (the input)

**(M8)** Cells add links to the chain without breaking it: a computed variable's `state.patch` has `cause` → the patch it read, so `why` passes through it to the input.

### 2.3 Project op log

Entities: `connection` (**(M9)** including composites, plugin `core.composite`), `surface`, `surface_group` (Later), `layer` (M5: `page`), `control_binding` (which also holds a control's helpers, replacing `value_mapping`), `control_group` (Later), `flow`, `variable` (**(M8)** declared, [dataflow §6](dataflow.md)), **(M9)** `component`, `placement` and `transform` ([composition](composition.md), [dataflow §4.4](dataflow.md)). Layers, composite connections and flows can be templates or instances of them, and placements are instances of components ([composition §11.2](composition.md#112-one-template-primitive-for-four-kinds)), `asset`, `user`, `role_assignment`, `setting`, `restore_point`.

```ts
interface ProjectOp extends Entry {
  kind: "entity.create" | "entity.update" | "entity.delete" | "entity.move"
      | "batch" | "restore" | "migrate";
  data: {
    entity: { type: EntityType; id: string };   // ULID
    baseRev?: number;                            // entity revision the client saw
    rev: number;                                 // revision after applying
    set?: { [fieldPath: string]: unknown };      // field-level sets (update/create)
    unset?: string[];
    inverse: { set?: {...}; unset?: string[] };  // for undo
    batch?: number;                              // the `batch` op's seq, when part of one (§7.6)
  };
}
```

**Conflict rule:**
1. If `baseRev` matches the current revision, apply the op.
2. Otherwise, apply only the fields that weren't changed since `baseRev`.
3. Overlapping fields: last write wins, and the client whose value was overwritten is notified.

**Undo** applies `inverse`, adjusted to the current state of the fields involved.

**Compaction:** periodically fold the ops into an entity snapshot. Keep ops after the newest restore point, plus a configurable history window.

### 2.4 What's kept, and for how long

**(M8)** The journal holds two tiers of entry, and only one of them has to reach the disk.

- **Inputs** can't be recomputed: patches from connections and from `set` steps, control input, action requests and results, run lifecycle and steps, `nondet`, `event`s from plugins, `project.applied`, connection status, node roles. They're written to the `journal` table and kept for the ring buffer's retention.
- **Derived entries** can be recomputed from the inputs and the project, given time: patches from cells, helpers and layer resolution (origin `{core: "dataflow"}`), with their `reducer` state, `trigger.matched`, `instance.added` and `instance.removed`, and events derived from input state (an `event` with a `surface`, or with origin `{core: "dataflow"}`). They're kept in the in-memory ring only, for its window (default 1 h, or 256 MB, whichever comes first). A sink's `action.requested` is an input, though its origin is `{core: "dataflow"}`: it's a side effect.
- **One `seq` for both.** The live stream (subscribers, the UI and the standby) has no gaps. The `journal` table has gaps where derived entries were, and `GET /journal` read from the table, past the memory window, says so with the header `Covalence-Derived-Dropped: true`. A derived entry's seq isn't reused after a restart: the store's `meta` reserves seqs 1024 at a time as derived entries take them, and the journal continues after the higher of that and the last stored entry.
- **`inputCause` keeps `why` working on disk.** An entry whose `cause` is a derived entry also records the nearest input among its causes. A sink's `action.requested` has `cause` → the cell's patch (derived) and `inputCause` → the control's input patch, and a `run.started` has `cause` → `trigger.matched` and `inputCause` → the event or patch it matched. Inside the memory window `why` follows `cause`, through every link. Past it, a chain follows `inputCause` over the derived entries: "the action came from this run, which this key press started". **(Later)** Replaying the journal from the nearest snapshot rebuilds the derived entries for an older window on request.
- **High-rate values are sampled** by their schema's `journal: {sampled}` flag ([dataflow §5.1](dataflow.md)), so a meter or timecode writes a few entries per second, not hundreds. Sampling applies to both tiers.
- **Inputs are written in batches:** one SQLite transaction per turn, or with `journal.flushInterval` set, at most one per interval. The default is per turn on a desktop and `100ms` on a Raspberry Pi, where derived entries never touch the SD card. A crash loses at most one interval of inputs from disk. A standby has them already, and `state_cache` is written in the same transaction.
- **Replication is unaffected.** The standby is sent the live stream, both tiers, not the disk (§7.3). One that's further behind than the memory window resyncs from a state snapshot ([dataflow §5.2](dataflow.md)).

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
  6. **(M8)** Resume reducers from their cells' `reducer` metadata and evaluate stateless cells once, with no sink firing ([dataflow §5.2](dataflow.md)).
- **Fencing:** hosts and Orbitals reject `action.invoke` messages carrying a term lower than the highest term they've seen.

---

## 5. Persistence layout (SQLite)

| Table | Contents |
|---|---|
| `entities` | `(type, id, rev, body JSON)`: the materialised project |
| `project_ops` | The project op log |
| `snapshots` | Restore points: a compacted copy of `entities` plus metadata |
| `journal` | The runtime journal (time- and size-limited) |
| `state_cache` | The last known state tree, so the UI shows `stale` values right after a restart. **(M8)** Persisted variables' values and `reducer` metadata are restored from it ([dataflow §6](dataflow.md)). M8 keeps only those, keyed by the variable's ID, and writes them as they change rather than with the journal's batch |
| `assets` | Content-addressed blobs (icons, images) |
| `meta` | `schemaVersion`, node ID, current term, **(M8)** `seqReserved`: seqs below it are taken (§2.4) |

Secrets aren't stored here (§7.2).

---

## 6. Open questions

1. Should integers be a distinct type in expressions, or should we always use f64 in JS and keep `int` only as a schema hint? (Leaning towards the hint.)
2. ~~Journal retention defaults on a Raspberry Pi with an SD card. How much should we write, and how do we limit wear?~~ Decided in §2.4: only inputs reach the disk, batched, with high-rate values sampled; derived entries stay in memory.
3. Should `run.checkpoint` be automatic at every step, or opt-in per flow? Automatic is simpler; opt-in costs less.
4. Should state patches from a connection be journaled in full, or sampled for very high-rate values (meters, timecode)? Proposal: the schema flag `journal: sampled(100ms)`. **(M8)** Decided for cells and helpers: `journal: {sampled: "100ms"}` applies every value to state, journals the latest per interval, and a final entry when it settles ([dataflow §5.1](dataflow.md)). Connections' schemas get the same flag when a plugin needs it.
5. ~~How should drafts interact with multi-user editing? Is one draft shared per layer, or does each user have their own?~~ Decided in §7.6: there are no drafts. Edits apply as they're saved, and an edit that spans entities is one batch.

---

## 7. Decisions

Why the core keeps state and history this way, and what was rejected. Keep these notes when the sections above change.

### 7.1 An event-sourced core with one output gate
- **Decided:** two logs (§2): the project op log for every configuration change (durable, attributed, replicated) and the runtime journal for what happened (high volume, limited retention). Every entry has a `seq`, a leadership `term`, a `ts`, an `origin` and a causation chain. **Every side effect passes one output gate** (§3), which journals `action.dispatched` or `action.suppressed`. Non-deterministic inputs (time, random numbers, action results) are journaled, so a session can be replayed exactly.
- **Why:** redundancy, multi-user editing, undo, history, the timeline ("why did this fire?") and replay all need the same two things: a numbered, ordered record of what happened, and one place where outputs can be controlled. Standbys replicate the logs instead of recomputing, multi-user editing is ordered operations, and the timeline reads the journal directly. The rule that follows: no subsystem, driver or plugin sends output except through actions the core requested.

### 7.2 Storage: SQLite, a text export, migrations
- **Decided:** the working store is SQLite in WAL mode (§5): entities, the op log, snapshots and restore points, assets and the journal.
  - **Text export:** a deterministic folder of YAML, one file per entity, with stable key order and IDs, importable losslessly. It's for git and code review, and needs round-trip tests to stay lossless.
  - **Bundle:** a `.covalence` zip of the export, assets and, optionally, the pinned plugin packages, for sharing.
  - **Secrets** live apart: the OS keychain where there is one, otherwise an encrypted file. Exports and bundles only reference them.
  - **Versioning:** the project records `schemaVersion` and the app applies ordered migrations. A project from a newer version opens read-only, with a warning. Each plugin versions its own config schema and ships upgrade functions.
  - **History:** the op log (attribution and time travel, compacted over time), named restore points (by hand, and automatically before plugin upgrades and on a schedule), and optional built-in git that commits the export on a schedule or by hand.
- **Why:** project data must survive a power cut mid-save, support multi-user editing, undo and replication from one mechanism, diff in git, travel as one file, and keep secrets out of shared copies.
- **Rejected:** a single JSON or YAML file (noisy diffs, no atomic partial writes); a folder of files as the source of truth (no atomic multi-file writes, poor for multi-user editing); TOML (awkward for deep nesting).

### 7.3 Execution model: one turn thread writes state
- **Decided:** one dedicated OS thread, the **turn thread**, is the only writer of state. Each turn it takes everything waiting on its input channel; applies it; evaluates cells in dependency order, then layer fields and resolution, then display bindings; matches triggers; and sends looks to the drawing thread, patches to subscribers and the journal, and sink actions to the gate. It has its own QuickJS runtime for pure evaluation only, with tight limits (2 ms per evaluation by default), and owns its data without locks: other threads read snapshots and the change feed. Beside it:

  | Work | Where |
  |---|---|
  | Applying patches, cells, reducers, transforms, bindings, layer resolution, trigger matching, `waitUntil` conditions | Turn thread |
  | Flow runs, waits, action dispatch | Tokio |
  | Script steps, expressions in action parameters | Script pool: QuickJS runtimes, one per thread (cores, at most 4), reading a snapshot taken when the step starts |
  | Drawing key images | Drawing thread, which drops a frame replaced before it was drawn |
  | Plugin host, Orbitals, devices, API, WebSockets | Tokio |

- **Why:** the work has two shapes. Continuous, pure work (patches, cells, bindings, layers) must be fast, ordered and deterministic for replay. Sequential, slow work (flows, device calls, scripts) has looser limits. With one QuickJS runtime behind a lock, a script step could hold up a key redraw. Split this way, a slow script can't delay a key, the 16 ms budget covers one turn and one draw, and benchmarks run against the turn. Script steps see a snapshot, not live state, as runs already do between steps.
- **Failover rule:** everything that matters for takeover is a patch on the turn thread's output, so a standby that applies the journal has it. Reducers keep no state outside their cell's value and the state they declare.
- **Later:** reuse contexts and compiled bindings in the turn's runtime, once benchmarks show it matters.
- **Rejected:**
  - *One runtime behind a lock:* any script step can block a redraw.
  - *Everything on tokio, the state tree behind a lock:* contention on every patch, and no fixed order within a turn for replay.
  - *A QuickJS runtime per flow or binding:* memory grows with the project, too much for a Raspberry Pi.
  - *Drawing on the turn thread:* a burst of changes on a 32-key deck spends the turn drawing.

### 7.3.1 Journal tiers
- **Decided:** inputs reach the disk and derived entries stay in memory (§2.4), with one `seq` for both. An entry whose `cause` is derived records `inputCause`, so `why` past the memory window steps over the gap. `GET /journal` flags a range read from disk with a header, and derived entries reserve seqs in blocks so a restart doesn't reuse them.
- **Why:** derived entries are most of the journal at show rate (every cell, helper and resolution change, every control event), and they can be recomputed. On a Raspberry Pi the SD card is the limit. `inputCause` keeps the one question that matters past the window: which input led here. A state leaf restored from `state_cache` names its patch's seq, so a reused seq would make `why` lie.
- **Rejected:**
  - *Storing everything and trimming sooner:* the window for inputs shrinks with the rate of derived entries.
  - *A separate seq for derived entries:* two orders to merge in the live stream and for the standby.
  - *`{derivedDropped: true}` in the body of `GET /journal`:* it would change the answer from a list of entries to an object for every client. A header says the same.
  - *Writing the seq high-water mark with every derived entry:* the disk write tiers are meant to avoid.

### 7.4 Redundancy and failover, in three levels (designed, not built)
- **Decided:** three opt-in levels.

  | Level | What the backup does | Takeover |
  |---|---|---|
  | 1. Config mirror | Keeps an up-to-date copy of the project | Manual |
  | 2. Warm standby | Mirrors the project and live state, keeps allowed device connections open, sends no outputs | One click |
  | 3. Hot standby | As 2, also mirrors in-flight runs and their checkpoints | Automatic when the heartbeat is lost |

  The active node streams both logs to the standby, which applies them without recomputing (§4). **Automatic failover needs a quorum from a witness** (any Orbital, or a small witness process); without one, takeover is manual. A term on every output rejects outputs from an old leader. Plugins declare `connections: multiple` (the standby connects and observes) or `single` (it stays disconnected until promoted), and actions marked `idempotent: true` must be safe to retry, keyed by the call ID. Runs in progress follow a per-flow policy: `abort` (default), `resume` from the last checkpoint, or `restart`. Both nodes must agree on the time source (§7.7).
- **Why:** live shows can't stop, but a small show shouldn't pay for complexity it doesn't need: one node with no witness pays nothing.
- **Say this clearly in user docs:** surfaces survive failover only through Orbitals, which connect to both nodes and follow the leader. A surface plugged into the active node's USB is lost if that machine dies.

### 7.5 Multi-user editing (designed, not built)
- **Decided:** **the server is the single source of truth.** Clients send operations on the op log, each carrying the entity's base revision. The server orders, applies and broadcasts them. Conflicts resolve field by field: separate fields merge, an overlapping field is last-write-wins and the overwritten user is told. No CRDTs.
  - **Presence:** who's online, what they're viewing, a soft lock on what someone is editing.
  - **Undo** is per user, from inverse operations rebased onto the current state.
  - **Audit:** every change is attributed, and the timeline shows who changed what and when.
  - **Roles:** `admin` (everything), `programmer` (edit the project), `operator` (press keys and run flows), `viewer` (read only). Login: local accounts first, API tokens for automation, SSO later.
  - **Show lock:** read-only for everyone below admin, for the project or for single layers.
- **Why:** a producer, a technical director and a programmer may edit or run the same system at once, during a show.

### 7.6 Editing a running show (proposed; kept state settled in M8)
People edit during services: a key relabelled, a flow fixed, a camera added between songs. Edits apply live, as they're saved, with one rule per question rather than one per kind of entity:
- **An edit that spans entities is saved as one batch.** `POST /batch` takes a list of create, update and delete operations on any entities, each on its base revision. The core validates the project as it would be after all of them, then applies them as one `batch` op in one turn, or refuses the lot and applies none. So a change that's only valid together, such as a new required param on a template and its value on every instance, passes where either alone would be `badInstance`. The editor builds the batch: adding a required param asks for its value on each instance and saves the template and its instances together. The timeline shows a batch as one entry ("Ana changed 'Camera' and 3 instances"), and undo reverses it as one.
- **A run finishes on the version it started with.** It pins its flow's revision, recorded in `run.started`, and edits apply from the next run (flows §8.5). Old revisions are kept while a run uses them. Saving a change to a flow with active runs offers `cancelRuns`.
- **State is kept when it still fits**, by entity identity, and **an edit never sends an action**: dataflow §5.4. A reset can leave a device out of step with its cell until the next change; that's the price of edits never acting on the show by themselves. `forEach` instances are matched by key when the `key` expression changes.
- **Rejected:**
  - *Drafts published as changesets:* edits staged in a draft, rebased onto changes made meanwhile and published together. A batch gives the same all-or-nothing validation without a second copy of the project to keep, merge and explain. Edits that apply as they're saved are what people expect from show-control software.
  - *One entity per save:* multi-entity changes would pass through states that were never meant to exist, such as an instance whose params don't match its template, and `badInstance` would leave no order of saves that gets there.
  - *Accept broken instances and flag them:* a template edit mid-service could silently stop every camera using it.
  - *Require a default on every new param:* the default is a guess about each instance, applied without asking.
  - *Runs switching to the new version at the next step:* a run would mix two versions, and vanished step IDs would need case-by-case handling.
  - *Reset all state on every edit:* a relabel would unlatch a key mid-show.
  - *Keep state whenever the name matches:* a `toggle` replaced by a `counter` would start from `true`.

### 7.7 Time sources and scheduling (proposed)
- **Proposed:** a time-source abstraction in the core: the system clock (default, with NTP sync status shown), a monotonic clock for durations and waits, and later LTC (audio) and MTC (MIDI) as time-source drivers. The project has a **timezone**. Schedules use cron-style rules and one-off dates with explicit daylight-saving behaviour, and a missed-schedule policy per trigger: `skip` (default), `run-if-within-grace(duration)` or `run-late`. Schedules are evaluated on the active node only; the standby mirrors the next fire times. Timecode triggers ("at HH:MM:SS:FF", ranges, chase and freewheel) come in v1.x. State: `sys.time.*` (now, timezone, sync status) and `sys.timecode.*`.
- **Why:** schedules, timers and timecode cues are core production triggers, redundant nodes must agree on time, and machines sleep, drift and cross daylight-saving changes.
- **Open:** is timecode in the first release? Can an Orbital provide a time source, such as an LTC input at front of house?
