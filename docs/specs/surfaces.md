# Spec: Surfaces and layers

- **Status:** Draft v0.4
- **Related:** [ADR 0022](../adr/0022-surfaces-pages-and-the-web-surface.md), [ADR 0023](../adr/0023-key-rendering-and-binding-evaluation.md), [ADR 0024](../adr/0024-surfaces-as-two-way-devices.md), [bindings](bindings.md), [flows](flows.md), [plugin protocol §8](plugin-protocol.md), [state and event log](state-and-event-log.md)

A **surface** is a control device: a Stream Deck, a MIDI controller, a motor-fader panel, a GPIO box, a panel in a browser. It's a two-way device, not an input device ([ADR 0024](../adr/0024-surfaces-as-two-way-devices.md)):
- Its controls' input is **state** the device reports, such as a key being pressed or a fader's position.
- **Events** are derived from that state, such as a press, a double-press or a turn.
- The core sets the controls' **outputs**: a key image, an LED, a fader position, text.

A **layer** is a set of control bindings on the controls it covers. Which layer controls each control is computed from state (§5), so one concept covers M5's pages, shift keys, popovers, folders and regions of a deck. This spec covers the entities, controls, input state and events, helpers, layers, control groups, triggers, and the web surface's transport. How a control looks is in the [bindings spec](bindings.md).

A device is a surface when it has controls the core interprets or renders for. A device that only reports values or occurrences (a barcode scanner, a keyboard, a sensor) is a connection with state and events. Both trigger flows the same way.

Shapes use TypeScript notation. The source of truth is the Rust types in `covalence-core` ([ADR 0020](../adr/0020-project-entity-types-in-the-core.md)).

**Markers.** Unmarked rules are implemented (M5 Phases 1–4). **(Now)** is the reshaping in [M5 Phase 5](../milestones/m5-surfaces-and-bindings.md), which replaces pages, the `k`-grid, `input.raw` and the `gesture` shapes before M5 closes. **(Later)** has no milestone yet.

---

## 1. Entities

```ts
interface Surface {               // project entity `surface`
  id: string;                     // ULID
  name: string;                   // "FOH deck"
  slug: string;                   // state key: surface.<slug>
  model: string;                  // "virtual.deck15"; hardware models later
  serial?: string;                // matches a hardware device on attach (later)
  startLayer?: string;            // (Now) layer ID shown in the `main` group on first attach (M5: `startPage`)
  provider?: string;              // (Later) the connection whose device this is
}

interface Layer {                 // (Now) project entity `layer`, replacing `page`
  id: string;
  name: string;                   // "Cameras"
  slug: string;                   // state key: layer.<slug>
  controls: LayerControl[]        // (Now) the controls it covers (M5: `layout: {cols, rows}`)
          | { region: Rect }      // (Later) every control inside a rectangle
          | { expr: Expr };       // (Later) control IDs from state
  on?: string[] | { expr: Expr }; // (Later) surfaces or surface groups it may appear on
  when?: When;                    // (Later) §5.3; default { nav: "main" }
  level?: "under" | "normal" | "over";   // (Later) §5.3; default "normal"
  rank?: number | { expr: Expr }; // (Later) within the level; default 0
  parent?: string;                // (Later) the tree preset, §5.3
}

interface LayerControl {          // (Now)
  id: string;                     // matches a ControlDef.id on the surfaces it fits
  at?: Placement;                 // where the editor draws it
  inputs?: InputKind[];           // capabilities this layer needs the control to have
  outputs?: OutputKind[];
}

interface SurfaceGroup {          // (Later) project entity `surface_group`
  id: string; name: string; slug: string;
  members: { surface: string; offset: { x: number; y: number } }[];
}

// `control_binding` is in the bindings spec, §1. `control_group` is in §6.
```

- **Slugs** follow the state segment rule (`[A-Za-z_][A-Za-z0-9_]*`) and are unique per entity type, as for flows.
- **A surface entity exists before its device attaches.** The virtual surface is created in the UI. Hardware surfaces will be created on first attach, matched by `model` and `serial` after that (later milestone).
- **A layer is independent of surfaces.** The same layer can control controls on several surfaces at once, and its bindings are evaluated once for all of them.
- **(Now) A layer lists its controls.** Control IDs on a layer are unique, and there are at most 256. The editor's "New layer" for a surface copies that surface's controls with their capabilities.
- **(Later) A surface group** places several surfaces on one canvas, each offset into the group's coordinates, so they work as one wide device. A layer `on` a group covers its controls by region, or by qualified ID `<surface slug>/<control id>`.
- **(Later) `provider`** ties a surface to the connection whose device it is.

---

## 2. Controls

A surface declares its controls when it attaches (§8, plugin protocol §8). **(Now)** A control is typed by what it can do, and has a position:

```ts
interface ControlDef {
  id: string;                     // unique per surface: "k0", "dial1", "pad.36"
  label?: string;                 // "Dial 1", for the editor
  at: Placement;                  // (Now) required
  inputs?: InputCap[];
  outputs?: OutputCap[];
}

type Placement = { x: number; y: number; w?: number; h?: number };   // grid cells; w and h default to 1
type Rect = { x: number; y: number; w: number; h: number };

type InputCap =
  | { kind: "momentary"; velocity?: boolean }   // down, up
  | { kind: "relative" }                        // turn: signed detents
  | { kind: "absolute"; axes?: 1 | 2 }          // value: 0..1, or [x, y]
  | { kind: "touch" };                          // touch: 1 or 0

type OutputCap =
  | { kind: "image"; w: number; h: number; format: string; rotation?: 0 | 90 | 180 | 270 }
  | { kind: "led"; rgb?: boolean }
  | { kind: "level"; motor?: boolean }          // motor fader, encoder ring, meter
  | { kind: "text"; chars?: number; lines?: number };

type InputKind = InputCap["kind"];
type OutputKind = OutputCap["kind"];
```

| Device | Inputs | Outputs |
|---|---|---|
| Stream Deck key | `momentary` | `image` |
| Stream Deck+ dial | `relative`, `momentary` | none (its strip is a separate `touch` + `image` control) |
| Motor fader | `absolute`, `touch` | `level` (`motor`) |
| MIDI pad | `momentary` (`velocity`) | `led` (`rgb`) |
| XY pad in a web panel | `absolute` (`axes: 2`) | none |
| GPIO input / tally output | `momentary` / none | none / `led` |

- **(Now) IDs are free-form.** The `k0…kN` row-major rule and the attach's `layout` are gone. The SDK keeps presets (`button`, `encoder`, `fader`, `touchstrip`, `lcd`, `led`) that produce common capability sets.
- **(Now) Every control has `at`.** Controls may not overlap. A device without a physical layout, such as GPIO pins, is placed in a row by its SDK preset. Positions are what regions, neighbours, surface groups and the editor's drawing are built on.
- **(Now)** A control with no inputs and no outputs is refused at attach, as is a duplicate ID or a missing `at`. Input that doesn't match a control's capabilities (a `turn` on a control with no `relative` input) is dropped and logged, rate-limited.
- **(Now) Outputs.** Only `image` does anything. **(Later)** `led`, `level` and `text` through the bindings' `led`, `value` and `text` properties, and surface-wide brightness.
- **(Later) Region and relative refs.** Wherever a control can be named (a layer's `controls`, a control group, a `ControlRef`), it can also be `{region: Rect}`, for every control whose `at` lies inside, or `{relative: "left" | "right" | "above" | "below", of: <control>}`, for the nearest control in that direction.

---

## 3. State

| Path | Value | Written by |
|---|---|---|
| `surface.<slug>.connected` | `boolean` | The core, on attach and detach |
| `surface.<slug>.model` | `string` | The core, on attach |
| `surface.<slug>.current.<group>` | layer slug, or `null` | **(Now)** The core and the API: the current layer of a navigation group (§5.3). `main` replaces M5's `surface.<slug>.page`, and is set on attach as `page` was |
| `surface.<slug>.layers` | layer slugs, highest first | **(Later)** The core: the active layers in resolution order (§5.2) |
| `surface.<slug>.controls.<id>.layer` | layer slug, or `null` | **(Later)** The core: the layer controlling the control (§5.2) |
| `surface.<slug>.controls.<id>.pressed` | `boolean` | **(Now)** The device: `momentary` input |
| `surface.<slug>.controls.<id>.velocity` | `number`, 0 to 1 | **(Now)** The device: the last `down`, if the capability has `velocity` |
| `surface.<slug>.controls.<id>.detents` | integer | **(Now)** The device: the running total of `relative` input since the attach |
| `surface.<slug>.controls.<id>.value` | `number`, or `[x, y]` | **(Now)** The device: `absolute` input |
| `surface.<slug>.controls.<id>.touched` | `boolean` | **(Now)** The device: `touch` input |
| `surface.<slug>.controls.<id>.requested.<output>` | the output's value | **(Later)** The core: what it last sent to a `led`, `level` or `text` output (§4.4) |
| `surface.<slug>.controls.<id>.helpers.<name>` | a helper's value | **(Later)** A helper with `scope: "surface"` (§4.3) |
| `layer.<slug>.controls.<id>.<name>` | a helper's value | **(Later)** A helper on a layer (§4.3) |
| `layer.<slug>.params.<name>` | a layer parameter | **(Later)** Bindings §6 |

- `surface` and `layer` are reserved roots: a connection alias can't be either (state spec §1.1). **(Now)** `layer` replaces M5's `page` root.
- Surface state is held while the surface is detached, with `connected: false`, so a binding that shows a surface's layer keeps working. Input state is the exception (§4.1).
- **Control IDs that aren't state segments** (`pad.36`) use bracket syntax in text paths: `surface.mixer.controls["pad.36"].pressed` (state spec §1.1).

---

## 4. Input

### 4.1 Input state

**(Now)** A `surface.input` (plugin protocol §8) is applied to the control's input state (§3) as a `state.patch`, with the surface as origin and the host's `ts` as `deviceTs`. It replaces M5's `input.raw` journal entry.

| Input | Changes |
|---|---|
| `down` | `pressed` to true, and `velocity` if declared |
| `up` | `pressed` to false |
| `turn` with `value: n` | `detents` by `n` |
| `value` | `value` |
| `touch` with 1 or 0 | `touched` |

- **No lost edges.** Each `down`, `up`, `turn` and `touch` is applied in a scheduler turn of its own. A tap shorter than a turn still gives two changes, and `state` triggers and bindings see each one. `value` input may be coalesced within a turn, with the latest value winning (state spec §1.5).
- **Device time.** `deviceTs` is the host's `ts`: monotonic per surface session, and never compared with the core's clock. Gesture timing uses it (§4.2). Ordering is by `seq`.
- **Volatile.**
  - **On attach,** input state is reset: every control released, `detents` at 0, and `value` absent until the device sends one.
  - **On detach,** every pressed control is released and every touched one untouched, with `cause` → the `state.patch` that set `connected: false`.
  - **After a restart,** input state isn't restored.
  - **After a failover,** the new active node starts with everything released ([ADR 0008](../adr/0008-redundancy-and-failover.md)).
- **Read-only.** Only the device writes input state. A `set` step only writes `local` and `vars` (flows §8.3), so flows inject input instead (§4.5).
- Input from a control the surface didn't declare is ignored and not journaled.

### 4.2 Events

**(Now)** Each input patch also gives **edge events**, journaled as `input.event` with `cause` → the patch, replacing M5's `input.gesture`.
- **Each event carries `layer`:** the layer that controlled the control when the input arrived (§5.2).
- **A timed `long` or `hold`** has `cause` → the patch for the `down` it follows, and carries the layer that controlled the control at that `down`.

| Event | Fires | Notes |
|---|---|---|
| `press` | `pressed` goes true | Straight away, never delayed by the double-press window |
| `release` | `pressed` goes false | Carries the layer of its `press`, even if another layer has taken the control since |
| `double` | `pressed` goes true within `doubleWindow` of it last going false | Fires **as well as** `press`. A flow that should act only on a double listens for `double` alone |
| `long` | Once, when a control has been pressed for `longAfter` | |
| `hold` | At `longAfter`, then every `holdEvery` while the control stays pressed | `delta` is the repeat count, from 1 |
| `turn` | `detents` changes | `delta` is the change |
| `change` | `value` changes | `value` is the new value. **(Later)** Not for echoes (§4.4) |
| `touch`, `untouch` | `touched` goes true or false | |

| Setting | Default |
|---|---|
| `doubleWindow` | 300 ms |
| `longAfter` | 500 ms |
| `holdEvery` | 150 ms |

- **Timing uses the device's `ts`**, so jitter between the device and the core doesn't change what a gesture is.
- **`long` and `hold` need a timer,** because no input arrives while a key is held. The core starts one when the `down` arrives. If the `up` arrives first and its `ts` shows the key was held for `longAfter` or more (late delivery), `long` fires before `release`. A `hold` repeat that would fire after the `up`'s `ts` doesn't fire.
- **Layer-relative input is events, not state.** Two surfaces can each be under one layer and each hold its `k0`, so `layer.<slug>` has no `pressed`. A `control` trigger on `{layer, control}` is matched per surface (§7).
- **(Later)** The timing settings are configurable per control with the `gestures` helper (§4.3).

### 4.3 Helpers (Later)

A **helper** builds on a control's input state to give derived state, events, or both. Helpers are configured per control on its `control_binding` (bindings §5). They make common cases quick, while input state and events stay available for anything else.

| Helper | Reads | Gives |
|---|---|---|
| `gestures` | `pressed` | the timed events in §4.2, with its own timing settings |
| `toggle` | `pressed` | a boolean, flipped on each `press` |
| `position` | `detents`, `value` | a number in `[min, max]`: step, wrap, acceleration, `follow` |
| `velocity` | `detents`, `value` | a rate in units per second, smoothed, decaying to 0 when the control stops |
| `select` | `detents` | an integer index, over a count or a list |
| `absolute` | `value` | the value through a curve, with pickup and `follow` |
| `rate` | `value` | −1 to 1 from the control's centre, with a deadband |
| `script` | everything above | whatever its expression returns: values and events |

- **Values** are written under `as` (the helper's name by default):
  - at `layer.<slug>.controls.<id>.<name>`, shared by every surface the layer is on
  - with `scope: "surface"`, at `surface.<slug>.controls.<id>.helpers.<name>` instead, one value per surface
  - in a variable, with `to: {set}`
  - into an action parameter, with `to: {action}`
- **Events** from a helper (a `toggle`'s `on` and `off`, a `select`'s `select`, a `script`'s own) are journaled as `input.event` with the helper's name, and match `control` triggers.
- **Options can be literals, refs or expressions,** so a step, a bound or a list can follow state.
- **`follow: Ref`** sets the value from state whenever that state changes. It pauses while the control is `touched`, and for `holdoff` (default 250 ms) after the last user input. **`pickup`** ignores a non-motor fader until it crosses the followed value.
- **`script`** is a sandboxed expression ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)) that gets the control's input state, the input that just arrived and its own previous values, and returns `{values?, events?}`. It runs on the binding thread with the bindings' 10 ms limit.
- **Helpers run in the core.** A plugin can suggest defaults for a control (`ControlDef.suggest`, plugin protocol §8). It can't run its own.

### 4.4 Reported and requested values (Later)

- **A two-way control has two values:**
  - what the device reports (`value`, `pressed` …)
  - what the core last sent (`requested.level`, `requested.led`, `requested.text`)

  Bindings and expressions can read both.
- **Echoes.** Input is an **echo** when either:
  - the host marks it (`echo: true`)
  - it arrives within `echoWindow` (default 100 ms) of an output to the same control, and matches it

  An echo updates input state with `cause` → the output's patch. It gives no `change`, `press` or `release` unless the trigger opts in (`echo: true` on a `control` trigger).
- The loop guard (flows §8.6) is the backstop for loops the echo rule misses.

### 4.5 Injecting input (Later)

- **A core action injects input** on a control as if the device had sent it: `down`, `up`, a `turn` or a `value`. It goes through §4.1 and §4.2 unchanged, with the flow run (or the UI user) as origin.
- This covers "press button X" macros, virtual buttons in the UI, and testing a layer without hardware.
- Its exact shape waits for core-provided actions (open question 1).

---

## 5. Layers

### 5.1 What a layer is

A layer covers some controls and binds them. A layer that covers every control of a surface is what M5 called a page. One that covers three keys is a shift layer, a popover or a section. **(Now)** Every layer is in the `main` navigation group, and the surface's current layer controls the controls it covers, which is M5's behaviour.

### 5.2 Who controls a control (Later)

For each surface and each control, the **controlling layer** is chosen from the layers that:
- cover the control (`controls`, evaluated for this surface)
- may appear on this surface (`on`)
- are active (`when`, evaluated for this surface)

The highest `level` wins, then the highest `rank`. A tie goes to the layer that became active most recently, then to the oldest layer (by ID). A control no layer covers is blank and ignores input.

- **The controlling layer sets the control's look and gets its input.** Nothing is resolved per property, and nothing is inherited from layers underneath.
- **A layer becomes active** when its `when` goes from falsy to truthy, or when navigation selects it (§5.3). Becoming active again moves it to the top of its level and rank.
- **Activation order is state:**
  - `surface.<slug>.layers` lists the active layers, highest first.
  - `surface.<slug>.controls.<id>.layer` names the controlling layer.

  Both are journaled, so `why` explains who controls a key. After a restart, the order is rebuilt from current state, with ties going to the oldest layer.
- **`self`.** In a layer's `controls`, `on`, `when` and `rank` expressions, `self` is the surface being resolved: `self.controls.k15.pressed`, `self.current.main`.
- **Guards.** A layer's expressions can't read resolution results (`surface.<slug>.layers`, `.controls.<id>.layer`); saving such a layer fails with `resolutionCycle`. Resolution runs once per scheduler turn, after bindings' reads have settled.
- **Armed layers.** A layer whose `when` reads input state is evaluated and rendered ahead of time while inactive, so that holding shift redraws within the 16 ms budget.
- **The editor shows a control's candidates:** "k3: Shift (over, while k15 is held), otherwise Cameras (main)". It can simulate a state ("hold k15") to show what would change.

### 5.3 Presets

Presets fill in `when`, `level`, `rank` and `controls` for common structures. As with structured bindings ([ADR 0023](../adr/0023-key-rendering-and-binding-evaluation.md)), they're stored as presets, evaluated natively, and shown with the expression they compile to. "Edit as expression" is one-way.

```ts
type When =
  | { nav: string }               // a navigation group: active while it's the group's current layer
  | { always: true }
  | { while: Expr }               // active while the expression is truthy
  | { expr: Expr };               // anything else
```

| Preset | Fields | Compiles to |
|---|---|---|
| **Navigation group** (default) | `when: {nav: "main"}` | `self.current.main == me`. Navigation sets `surface.<slug>.current.<group>`; folders are layers in one group with navigation keys between them |
| **Levels** | `level: under \| normal \| over` | `rank` bands of 0, 100 and 200, with `rank` (0 to 99) inside the band |
| **Shift** | `level: over`, `when: {while: self.controls.k15.pressed}` | Active while the key is held. A latching shift uses a `toggle` helper's value |
| **Popover** | `level: over`, `when: {nav: "popover"}` | Opened by navigating the `popover` group to it, dismissed by navigating it to `null` |
| **Tree** | `parent: <layer>` | `when` is combined with the parent's (both must be active). `rank` follows position among siblings, so a later sibling covers an earlier one |
| **Section** | `controls: {region}`, `when: {nav: "<section>"}` | A region of the deck with its own current layer, as in Bitfocus Buttons |
| **Always on** | `when: {always: true}`, usually `level: under` | A strip that shows wherever nothing else covers it |

- **Navigation** is an API call (§9) and, later, a core action. Each group keeps a short history for "back".
- **(Later) Layer parameters** (bindings §6) are readable by the layer's bindings, and with the tree preset, by its descendants.

### 5.4 Fit

- **(Now) A layer fits a surface** when every one of its listed controls exists on the surface with at least the inputs and outputs the layer declares for it. The surface may have controls the layer doesn't.
- A layer that doesn't fit can't be navigated to on that surface (`doesNotFit`, §9), which replaces M5's equal-layout rule.
- **(Later)** A layer covering a `region` or an `expr` always fits. It covers whatever controls are there.
- **(Later) Placement per surface** maps a layer's controls onto a surface of a different shape.

---

## 6. Control groups (Later)

A **control group** spreads a live list over a set of controls in a layer: a grid of ISO values for the selected camera, a row of inputs on a router, a list of cues.

```ts
interface ControlGroup {          // project entity `control_group`
  id: string;
  layer: string;
  controls: string[] | { region: Rect };   // the controls it owns
  items: Expr;                    // target(vars.selectedCam).iso.options
  key?: Expr;                     // each item's identity; default the item itself
  props: { [prop in ControlProp]?: Binding };   // `item`, `index` and `count` in scope
  helpers?: Helper[];
  overflow?: "clip" | "scroll";   // scroll: the core holds an offset; keys outside the group move it
}
```

- **The group owns its controls.** A plain binding on one of them, or two groups that overlap, is `duplicateControl`. Controls with no item are blank.
- **Bindings see the item.** A label of `{item.label}`, and a `rules` binding on `background` that's red when `item.value == target(vars.selectedCam).iso.current`, give the current-setting indicator.
- **Triggers see the item.** A `control` trigger on `{group}` fires for any control in the group, with `trigger.item`, `trigger.index` and `trigger.key` the item the control showed at the `press`. One flow handles the whole grid.
- **`overflow: "scroll"`** keeps the offset in `layer.<slug>.groups.<group>.offset`. Flows or keys outside the group move it.
- **Nothing is generated.** A group is one saved entity evaluated over a live list. The core never creates bindings at run time.

---

## 7. Triggers

Flows can start from a control in two ways (flows §1.1). **A control's binding holds its look and its helpers. What happens when it's used lives in flows**, and the key editor shows every flow that references a control (flows §6).

**`control` triggers** are the quick way in, for events:

```ts
{ kind: "control", control: ControlRef, event: string, echo?: boolean }   // (Now) replaces `gesture` triggers

type ControlRef =
  | { layer: string; control: string }      // the control on any surface where the layer controls it
  | { surface: string; control: string }    // (Later) whatever layer controls it
  | { group: string }                       // (Later) any control in a control group (§6)
  | { layer: string; region: Rect }         // (Later) any control in a region of a layer
  | { layer: string; relative: "left" | "right" | "above" | "below"; of: string }   // (Later)
  | { select: ControlSelector }             // (Later) e.g. every relative control on a layer
  | { expr: Expr };                         // (Later) resolved when the input arrives
```

- **(Now)** `event` is one of the events in §4.2. A `{layer, control}` trigger matches that control **on any surface where that layer controlled it** when the input arrived. `layer` is a layer ID; the editor shows its name.
- **The `trigger` payload** is `{surface, layer?, control, event, delta?, value?}`, plus **(Later)** `item`, `index` and `key` for a group. `surface` and `layer` are entity IDs, as an `event` trigger's `source.conn` is.
- `trigger.matched` has `cause` → the `input.event` entry and the surface as its origin, so `why` on an action reaches the input's patch.
- **Validation:**
  - The layer must exist (`unknownLayer`), and `control` must be one of its listed controls (`unknownControl`).
  - An event the control's inputs can't give (a `turn` on a key) is `unsupported`.

**`state` triggers** work on input state as they do on any state, for conditions and combinations:
- A press only while armed: `when: surface.deck.controls.k0.pressed && vars.armed`, `edge: rising`.
- A chord: `when: surface.deck.controls.k3.pressed && surface.deck.controls.k4.pressed`, `edge: rising`.
- A helper's value: `when: layer.cams.controls.dial1.position > 0.5`, `edge: rising`.
- **(Later) Pattern `state` triggers** match many paths at once: `match: "surface.*.controls.*.pressed"`, with the payload `{path, captures, value, previous, ts}` (flows §1.1). They work on any connection's state the same way.

A shift key is usually a layer (§5.3) rather than a condition, so the deck shows what the shifted keys do and the editor has a key to select.

---

## 8. The web surface

The browser's virtual deck is a surface like any other ([ADR 0022](../adr/0022-surfaces-pages-and-the-web-surface.md)). It connects to `GET /surface/{id}`, a WebSocket that carries the plugin protocol's `surface.*` notifications (plugin protocol §8) as JSON text frames:

```json
{ "method": "surface.attach", "params": { "surface": "<slug>", "model": "virtual.deck15", "name": "…", "controls": [
  { "id": "k0", "at": { "x": 0, "y": 0 }, "inputs": [{ "kind": "momentary" }],
    "outputs": [{ "kind": "image", "w": 144, "h": 144, "format": "png" }] }, … ] } }
{ "method": "surface.input",  "params": { "surface": "<slug>", "control": "k3", "kind": "down", "ts": 1234.5 } }
{ "method": "surface.draw",   "params": { "surface": "<slug>", "control": "k3", "format": "png", "image": "<base64>" } }
```

- **(Now)** The virtual deck declares 15 controls `k0`…`k14`, each `momentary` with a 144 × 144 PNG `image`, placed 5 × 3 with `at`. The attach has no `layout`.
- **One surface per socket.** The socket's `{id}` must name an existing surface entity, or the upgrade is refused with `404`. The first message must be `surface.attach` within 10 s, and it and every `surface.input` must use the surface's slug.
- **Errors close the socket** with code 1008 (policy) and the reason as the close frame's text:
  - a message that isn't a notification
  - an attach the core refuses (`cannot attach: …`)
  - another surface's slug
  - a method other than `surface.input` and `surface.detach` after the attach
- **A second attach for the same surface replaces the first,** whose socket is closed with 1000 and `attached elsewhere`. Two browser tabs can't drive one surface. Deleting the surface closes it with 1000 and `surface deleted`; shutting down, with 1001.
- **`ts`** is the browser's `performance.now()` in milliseconds.
- **Images are base64 PNG** at the size each control's `image` output declares. The virtual deck declares 144 × 144 so keys stay sharp on high-density screens.
- **The socket closing is a detach.**

---

## 9. HTTP API

Surfaces, layers and control bindings have the same routes and answers as flows (flow spec §9): `POST` creates and assigns the ID, `PUT /{id}` replaces, each answers `{surface | layer | binding, warnings}` or `422 {errors, warnings}`, and a missing slug is derived from the name. **(Now)** `/layers` replaces M5's `/pages`.

| Method | Path | |
|---|---|---|
| `GET` | `/surfaces`, `/layers`, `/control-bindings?layer=<id>` | List |
| `POST` | `/surfaces`, `/layers`, `/control-bindings` | Create, validated like flows |
| `GET`, `PUT`, `DELETE` | `/surfaces/{id}`, `/layers/{id}`, `/control-bindings/{id}` | Read, replace, delete. Deleting a surface detaches it and deletes its state. Deleting a layer deletes its bindings and clears it as any surface's `startLayer`; a flow whose `control` trigger names it gets `unknownLayer` when it's next saved |
| `GET` | `/surfaces/{id}/controls` | **(Now)** The controls the surface declared when it last attached, for "New layer" in the editor. `404` if it has never attached |
| `POST` | `/surfaces/{id}/current` `{group?: "main", layer: <id> \| null}` | **(Now)** Navigates a group (only `main` until groups arrive) and answers `{surface, group, layer}` once the surface has been sent its controls' outputs. Journaled as a `state.patch` on `surface.<slug>.current.<group>`, origin the user. `422 {error, code: "unknownLayer"}`, or `409 {error, code: "doesNotFit", controls}` listing the layer controls the surface lacks or can't support. Replaces M5's `POST /surfaces/{id}/page` and `layoutMismatch` |
| `GET` | `/control-bindings/problems` | `[{binding, prop, message}]`: each binding property that failed when last evaluated (bindings §3) |
| `POST` | `/control-bindings/preview` `{binding, size?}` | The editor's preview (bindings §2.2) |
| `GET` | `/surface/{id}` | The web surface WebSocket (§8) |

- **Validation:**
  - **A surface** needs a `name`, a `model` and a slug no other surface has (`badSlug`, `duplicateSlug`), and its `startLayer` must exist (`unknownLayer`).
  - **A layer** needs a `name` and a unique slug. **(Now)** Its controls need unique IDs (`duplicateControl`), there are at most 256, and every control bound on the layer must still be among them (`unknownControl` at `controls`).
  - **(Later)** A layer's expressions can't read resolution results (`resolutionCycle`).
  - **Bindings** are validated as in bindings §2.1.
- **Edits answer once drawn.** Saving or deleting a layer or binding, and replacing a surface, answer after surfaces showing the change have been sent their controls' outputs.

- **(M5)** Flows can't navigate or inject input yet. Both need actions on the core itself, which wait for a design of core-provided actions.

---

## 10. Open questions

1. Core-provided actions (navigate a group, back, set brightness, inject input) for flows: a reserved `core` target, or new step kinds? A surface's outputs would be natural targets, as in `target(vars.deck).setBrightness`.
2. ~~Mapping a page onto a surface of a different size~~: layers list their controls and fit by capability, or cover a region (§5.4). Explicit placement per surface is Later.
3. Should hardware surfaces that attach for the first time get an entity automatically, or wait in an "unassigned" list until a user adds them?
4. Which surface interfaces (`surface.keypad@1`, `surface.mixer@1` …) should exist, and do they name controls by role so one layer can target any surface of a shape?
5. Should a layer control be able to ask for a capability range (an image of at least 72 × 72) rather than just a capability kind?
6. **Capabilities beyond the four inputs and four outputs:** pressure and aftertouch, more than two axes, multi-position switches, haptics, sounds. Should unknown kinds attach with their input written as raw state under the kind's name?
7. **Whole-surface canvases:** a Loupedeck or tablet screen with touch zones, drawn as one image rather than per control.
8. **Time-based output:** blinking and animation. Bindings follow state, not time.
9. **Sequences** ("A then B within 1 s"): these need recent history, not current state.
10. **Orbitals while the core is unreachable:** should an Orbital keep showing the last outputs, or run some feedback locally?
11. **Reading the look underneath:** a shift layer that relabels a key has to set its colour too. Should a binding be able to read the look the next layer down would give (`below.background`)?
