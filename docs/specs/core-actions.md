# Spec: The core connection

- **Status:** Draft v0.1
- **Related:** [surfaces](surfaces.md), [flows](flows.md), [plugin protocol §4](plugin-protocol.md), [dataflow](dataflow.md)

Everything Covalence itself can do (navigate a surface, inject input, cancel a flow) is an action on a built-in connection, `core`. Flows, sinks, composites and the API use it exactly as they use a plugin's actions, with the same gate, journal, `why` and typing.

**Markers.** **(M6)** is [M6](../milestones/m6-primitives.md). **(M7)** is [M7](../milestones/m7-surfaces-from-plugins.md). **(Later)** has no milestone yet.

---

## 1. The connection (M6)

- **ID and alias `core`.** Both are reserved: no connection can take them, and `core` is reserved as a state root, so an alias can't shadow it.
- **It has no state subtree of its own.** Its state is the core's roots: `sys`, `surface`, `component`, `flow` and `vars`.
- **It always exists and is always connected.** It can't be created, edited, disabled or deleted, and it's listed with the connections in the API, marked `builtIn: true`.
- **Its manifest is served like a plugin's,** so the editor validates `core` actions, parameters and results the same way. It's listed in `GET /plugins` with the ID `core`.
- **Actions run in-process.** They never cross the plugin host, and they answer in the same scheduler turn unless they wait for a surface to be drawn.

## 2. Surfaces and flows are parameters

A target is a connection and nothing within it ([plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed)), so every `core` action names the surface or flow it acts on in a parameter, by ID:

| Parameter | Schema | Actions |
|---|---|---|
| `surface` | `{type: "string", format: "surface"}` | `navigate`, `inject` |
| `flow` | `{type: "string", format: "flow"}` | `cancelRuns`, `setEnabled` |

- **It's an ID, not a slug,** so renaming a deck or a flow never breaks a step. The editor shows names and stores IDs; a literal that names nothing is `unknownTarget` on save, and at run time the action fails with `unknownSurface` or `unknownFlow`.
- **It can follow state like any parameter** (dataflow §1). "Navigate the selected deck" is `navigate {surface: {ref: vars.deck}, component}`, with `vars.deck` holding a surface ID.
- **The `format` is what marks it,** in the manifest the core serves, so the editor offers a surface or flow picker and validation knows what to check. The actions are defined in `covalence-core`, beside the core connection.
- Surfaces from plugins (M7) are surfaces like any other: `navigate` names them by surface ID, not through their provider connection.

## 3. Actions

### 3.1 On a surface

| Action | Params | Result | Effect | Notes |
|---|---|---|---|---|
| `navigate` | `{surface: string, group?: string = "main", component: ComponentId \| null, item?: PathSegment}` | `{component, item?}` | internal | **(M6)** As `POST /surfaces/{id}/current` (surfaces §9): answers once the surface has been sent its outputs. **(M9)** `component` was `layer`, and must be a shown component. Errors `unknownSurface`, `unknownComponent`, `doesNotFit`, and **(M8)** `invalid` for a group that isn't a name. Idempotent. **(M9)** `item` picks one instance of a shown component with `forEach` ([composition §4](composition.md)); `unknownItem` if it has none by that key |
| `back` | `{surface: string, group?: string = "main"}` | `{component}` | internal | **(Later)** Pops the group's history |
| `inject` | `{surface: string, control: string, input: "down" \| "up" \| "turn" \| "value" \| "touch", value?: Value}` | none | internal | **(M6)** Applied as if the device had sent it (surfaces §4.5), with the run as origin. Errors `unknownSurface`, `notAttached`, `unknownControl`, `unsupported` |
| `setBrightness` | `{surface: string, level: number}` (0 to 1) | none | device | **(Later)** Needs surface-wide outputs |

### 3.2 On a flow

| Action | Params | Result | Effect | Notes |
|---|---|---|---|---|
| `cancelRuns` | `{flow: string}` | `{cancelled: number}` | internal | **(M6)** Cancels every active and queued run of the flow. Errors `unknownFlow` |
| `setEnabled` | `{flow: string, enabled: boolean}` | none | internal | **(M6)** Journaled as a project change too, with origin the run. Errors `unknownFlow` |

Running another flow is the `call` step, not an action: it can wait for the callee and get its outputs (flows §2.1).

## 4. The output gate (M6)

- **`ActionDef` gains `effect: "internal" | "device"`** (plugin protocol §4), default `device`.
- **`simulate` suppresses `device` actions only.** Navigating a deck or cancelling a flow still happens in simulate mode, which is what rehearsing needs.
- **Role and term apply to every action.** A standby node runs no flows, and a stale term is refused, whatever the effect.
- Every `core` action is journaled `action.requested` → `action.dispatched` or `action.suppressed` → `action.result`, as any other ([state spec §2.2](state-and-event-log.md)).

## 5. Open questions

1. Should `navigate` accept a selector of surfaces (every deck showing the page), or is a `forEach` loop over surface IDs enough?
2. Should project edits (`setEnabled`) be possible from flows at all while a show lock is on?

---

## 6. Decisions

### 6.1 The core is a connection
- **Decided:** a built-in connection, ID and alias `core`, always present and connected, owns everything Covalence itself can do. Its actions name a surface or a flow by parameter (§2), its state is the core's roots (with `core` reserved as a root), and `ActionDef.effect` (`internal` or `device`) lets simulate mode suppress only device effects, so a rehearsal still navigates decks. No new step kinds: `call`, `set`, `wait` and the blocks stay steps because they're about the run itself.
- **Why:** actions already have typed parameters and results, journaling, the output gate, simulate mode and idempotency. So flows navigate, inject and cancel with the step they already have, "navigate the selected deck" is a parameter that follows state, and sinks and composites can drive core actions (an encoder `select` reducer paging through pages).
- **Rejected:**
  - *New step kinds* (`navigate`, `inject`): each a one-off with its own editor, typing and gate rules.
  - *A `core` target that isn't a connection:* a special case everywhere connections are listed, manifests checked or targets resolved.
  - *One connection per surface:* surfaces already have a provider connection, and two for one device would confuse the UI.

### 6.2 Surfaces and flows are parameters, not items
- **Decided:** a `core` target is the connection, `{conn: "core"}`, and each action names its surface or flow in a `surface` or `flow` parameter, by ID, marked with `format` (§2). This replaces `core`'s two collections, `surfaces` and `flows`, whose items were targets `{conn: "core", key}` constrained by `{plugin: "core", collection}`.
- **Why:** surfaces and flows are in state (`surface.<slug>`, `flow.<slug>`), so a parameter can choose from them as a Videohub's `route` chooses an output. Parameters already follow state, so "the selected deck" needs no target kind of its own, and with no plugin needing collections (plugin protocol §13.9) the core would have kept a second way of addressing things only for itself.
- **Rejected:** *collections of items as targets* (`{conn: "core", key: <surface ID>}`): a key on every target, a collection on every constraint, and `key` on the journal and the core call, for something a parameter does.
