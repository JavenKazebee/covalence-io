# Spec: Surfaces and pages

- **Status:** Draft v0.1
- **Related:** [ADR 0022](../adr/0022-surfaces-pages-and-the-web-surface.md), [ADR 0023](../adr/0023-key-rendering-and-binding-evaluation.md), [bindings](bindings.md), [flows](flows.md), [plugin protocol §8](plugin-protocol.md), [state and event log](state-and-event-log.md)

A **surface** is a control device: a Stream Deck, a MIDI controller, a panel in a browser. A **page** is a layout of control bindings that a surface shows. This spec covers the entities, their state, how input becomes gestures, and the web surface's transport. How a control looks is in the [bindings spec](bindings.md).

Shapes use TypeScript notation. The source of truth is the Rust types in `covalence-core` ([ADR 0020](../adr/0020-project-entity-types-in-the-core.md)).

---

## 1. Entities

```ts
interface Surface {               // project entity `surface`
  id: string;                     // ULID
  name: string;                   // "FOH deck"
  slug: string;                   // state key: surface.<slug>
  model: string;                  // "virtual.deck15"; hardware models later
  serial?: string;                // matches a hardware device on attach (later)
  startPage?: string;             // page ID shown when the surface attaches
}

interface Page {                  // project entity `page`
  id: string;
  name: string;                   // "Cameras"
  slug: string;                   // state key: page.<slug>
  layout: { cols: number; rows: number };
}

// `control_binding` is in the bindings spec, §1.
```

- **Slugs** follow the state segment rule (`[A-Za-z_][A-Za-z0-9_]*`) and are unique per entity type, as for flows.
- **A surface entity exists before its device attaches.** The virtual surface is created in the UI. Hardware surfaces will be created on first attach, matched by `model` and `serial` after that (later milestone).
- **Control IDs.** A page's buttons are `k0`, `k1` … in row-major order across `layout`. A surface's button `ControlDef`s use the same IDs across its own grid. **(M5)** A page can be shown on a surface only if their layouts are equal; mapping across sizes comes later.
- **A page is independent of surfaces.** The same page can be shown on several surfaces at once, and its bindings are evaluated once for all of them.

---

## 2. State

| Path | Value | Written by |
|---|---|---|
| `surface.<slug>.connected` | `boolean` | The core, on attach and detach |
| `surface.<slug>.page` | page slug, or `null` | The core: `startPage` on attach, then the API (§5) |
| `surface.<slug>.model` | `string` | The core, on attach |

- `surface` and `page` are reserved roots: a connection alias can't be either (state spec §1.1).
- **(M5)** The `page` root is reserved but empty. Page parameters (bindings §6) will live there.
- Surface state is held while the surface is detached, with `connected: false`, so a binding that shows a surface's page keeps working.

---

## 3. Input and gestures

Every `surface.input` is journaled as `input.raw` (state spec §2). The core turns raw input into gestures and journals each as `input.gesture`, with `cause` → the `input.raw` entry that completed it.

| Gesture | Fires | Notes |
|---|---|---|
| `press` | On every `down` | Straight away, never delayed by the double-press window |
| `release` | On every `up` | |
| `double` | On a `down` within `doubleWindow` of the previous `up` on the same control | Fires **as well as** `press`. A flow that should act only on a double listens for `double` alone |
| `long` | Once, when a control has been down for `longAfter` | |
| `hold` | At `longAfter`, then every `holdEvery` while the control stays down | `delta` is the repeat count, from 1 |
| `turn` | Encoders (later) | `delta` is the detents turned |

| Setting | Default |
|---|---|
| `doubleWindow` | 300 ms |
| `longAfter` | 500 ms |
| `holdEvery` | 150 ms |

- **Timing comes from the host's `ts`**, so jitter between the device and the core doesn't change what a gesture is. `ts` is monotonic per surface session; it isn't compared with the core's clock.
- **`long` and `hold` need a timer,** because no input arrives while a key is held. The core starts one when the `down` arrives. If the `up` arrives first and its `ts` shows the key was held for `longAfter` or more (late delivery), `long` fires before `release`. A `hold` repeat that would fire after the `up`'s `ts` doesn't fire.
- **A detach releases everything:** controls that were down get `release`, with `cause` → the detach.

### 3.1 `gesture` triggers

```ts
{ kind: "gesture", control: { page: string; control: string }, gesture: Gesture }
```

- The trigger matches a gesture on that control **on any surface that's showing that page** when the input arrives. `page` is a page ID; the editor shows its name.
- The `trigger` payload is `{surface, page, control, gesture, delta?}`, with `surface` and `page` as slugs.
- `trigger.matched` has `cause` → the `input.gesture` entry, so `why` on an action reaches the raw input.
- Validation: the page must exist and `control` must be inside its layout (`unknownControl`).

---

## 4. The web surface

The browser's virtual deck is a surface like any other ([ADR 0022](../adr/0022-surfaces-pages-and-the-web-surface.md)). It connects to `GET /surface/{id}`, a WebSocket that carries the plugin protocol's `surface.*` notifications (plugin protocol §8) as JSON text frames:

```json
{ "method": "surface.attach", "params": { "surface": "<slug>", "model": "virtual.deck15", "name": "…", "controls": [ … ], "layout": { "cols": 5, "rows": 3 } } }
{ "method": "surface.input",  "params": { "surface": "<slug>", "control": "k3", "kind": "down", "ts": 1234.5 } }
{ "method": "surface.draw",   "params": { "surface": "<slug>", "control": "k3", "format": "png", "image": "<base64>" } }
```

- **One surface per socket.** The socket's `{id}` must name an existing surface entity, and `attach` must use its slug, or the core closes the socket with an error.
- **A second attach for the same surface replaces the first,** whose socket is closed. Two browser tabs can't drive one surface.
- **`ts`** is the browser's `performance.now()` in milliseconds.
- **Images are base64 PNG** at the size the attach declared for each control. The virtual deck declares 144 × 144 so keys stay sharp on high-density screens.
- **The socket closing is a detach.**

---

## 5. HTTP API

| Method | Path | |
|---|---|---|
| `GET` | `/surfaces`, `/pages`, `/control-bindings?page=<id>` | List |
| `PUT` | `/surfaces/{id}`, `/pages/{id}`, `/control-bindings/{id}` | Create or replace, validated like flows |
| `DELETE` | same | Deleting a page deletes its bindings. A flow whose `gesture` trigger names it gets a validation error |
| `POST` | `/surfaces/{id}/page` `{page: <id> \| null}` | Shows a page. Journaled as a `state.patch` on `surface.<slug>.page`, origin the user. Fails with `layoutMismatch` if the layouts differ |
| `GET` | `/surface/{id}` | The web surface WebSocket (§4) |

- **(M5)** Flows can't switch pages yet. That needs an action on the core itself, which waits for a design of core-provided actions.

---

## 6. Open questions

1. Core-provided actions (switch page, set brightness) for flows: a reserved `core` target, or new step kinds?
2. Mapping a page onto a surface of a different size: by position, with keys outside the surface unreachable, or by explicit per-surface placement?
3. Should hardware surfaces that attach for the first time get an entity automatically, or wait in an "unassigned" list until a user adds them?
