# Spec: The core connection

- **Status:** Draft v0.1
- **Related:** [ADR 0028](../adr/0028-the-core-is-a-connection.md), [ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md), [surfaces](surfaces.md), [flows](flows.md), [plugin protocol §4](plugin-protocol.md), [dataflow](dataflow.md)

Everything Covalence itself can do (navigate a surface, inject input, cancel a flow) is an action on a built-in connection, `core`. Flows, sinks, composites and the API use it exactly as they use a plugin's actions, with the same gate, journal, `why`, typing and dynamic targets.

**Markers.** **(M6)** is [M6](../milestones/m6-primitives.md). **(M7)** is [M7](../milestones/m7-surfaces-from-plugins.md). **(Later)** has no milestone yet.

---

## 1. The connection (M6)

- **ID and alias `core`.** Both are reserved: no connection can take them, and `core` is reserved as a state root, so an alias can't shadow it.
- **It has no state subtree of its own.** Its state is the core's roots: `sys`, `surface`, `layer`, `flow` and `vars`. `target({conn: "core", key})` returns the view of the item (§2).
- **It always exists and is always connected.** It can't be created, edited, disabled or deleted, and it's listed with the connections in the API, marked `builtIn: true`.
- **Its manifest is served like a plugin's,** so the editor validates `core` actions, parameters and results the same way. It's listed in `GET /plugins` with the ID `core`.
- **Actions run in-process.** They never cross the plugin host, and they answer in the same scheduler turn unless they wait for a surface to be drawn.

## 2. Collections and interfaces

| Collection | Key | Interface | `target(t)` is |
|---|---|---|---|
| `surfaces` | surface ID | `surface@1` | `surface.<slug>` |
| `flows` | flow ID | `flow@1` | `flow.<slug>` |

- **A variable can hold "the selected deck":** `schema: {kind: "target", interface: "surface@1"}` (state spec §1.2). Its value is `{conn: "core", key: "<surface ID>"}`.
- **Interfaces are maintained with the others** in `protocol/`. Surfaces from plugins (M7) are surfaces like any other, so they're items of `core`'s `surfaces`, not of their provider connection.

## 3. Actions

### 3.1 `surface@1`, on `{conn: "core", key: <surface ID>}`

| Action | Params | Result | Effect | Notes |
|---|---|---|---|---|
| `navigate` | `{group?: string = "main", layer: LayerId \| null, item?: PathSegment}` | `{layer, item?}` | internal | **(M6)** As `POST /surfaces/{id}/current` (surfaces §9): answers once the surface has been sent its outputs. Errors `unknownLayer`, `doesNotFit`. Idempotent. **(M9)** `item` picks one instance of a layer with `forEach` ([composition §4](composition.md)); `unknownItem` if it has none by that key |
| `back` | `{group?: string = "main"}` | `{layer}` | internal | **(Later)** Pops the group's history |
| `inject` | `{control: string, input: "down" \| "up" \| "turn" \| "value" \| "touch", value?: Value}` | none | internal | **(M6)** Applied as if the device had sent it (surfaces §4.5), with the run as origin. Errors `unknownControl`, `unsupported` |
| `setBrightness` | `{level: number}` (0 to 1) | none | device | **(Later)** Needs surface-wide outputs |

### 3.2 `flow@1`, on `{conn: "core", key: <flow ID>}`

| Action | Params | Result | Effect | Notes |
|---|---|---|---|---|
| `cancelRuns` | `{}` | `{cancelled: number}` | internal | **(M6)** Cancels every active and queued run of the flow |
| `setEnabled` | `{enabled: boolean}` | none | internal | **(M6)** Journaled as a project change too, with origin the run |

Running another flow is the `call` step, not an action: it can wait for the callee and get its outputs (flows §2.1).

## 4. The output gate (M6)

- **`ActionDef` gains `effect: "internal" | "device"`** (plugin protocol §4), default `device`.
- **`simulate` suppresses `device` actions only.** Navigating a deck or cancelling a flow still happens in simulate mode, which is what rehearsing needs.
- **Role and term apply to every action.** A standby node runs no flows, and a stale term is refused, whatever the effect.
- Every `core` action is journaled `action.requested` → `action.dispatched` or `action.suppressed` → `action.result`, as any other ([state spec §2.2](state-and-event-log.md)).

## 5. Open questions

1. Should `navigate` accept a selector of surfaces (every deck showing the layer), or is a `forEach` loop over `core` targets enough?
2. Should project edits (`setEnabled`) be possible from flows at all while a show lock is on?
