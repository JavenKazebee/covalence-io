# Spec: Surfaces

- **Status:** Draft v0.6
- **Related:** [dataflow](dataflow.md), [bindings](bindings.md), [flows](flows.md), [core actions](core-actions.md), [composition](composition.md), [plugin protocol §8](plugin-protocol.md), [state and event log](state-and-event-log.md)

A **surface** is a control device: a Stream Deck, a MIDI controller, a motor-fader panel, a GPIO box, a panel in a browser. It's a two-way device, not an input device (§11.2):
- Its controls' input is **state** the device reports, such as a key being pressed or a fader's position.
- **Events** are derived from that state, such as a press, a double-press or a turn.
- The core sets the controls' **outputs**: a key image, an LED, a fader position, text.

What's on a surface's controls is a **component** ([composition §7](composition.md#7-components)): one key, a strip or a whole page, made of smaller components. A component **shown** on surfaces is a candidate for its controls, and which one controls each control is computed from state (§5), so one rule covers M5's pages, shift keys, popovers, folders and regions of a deck. This spec covers the surface entity, controls, input state and events, helpers, showing components, triggers, and the web surface's transport. How a control looks is in the [bindings spec](bindings.md), and how components are built from others in the [composition spec](composition.md).

A device is a surface when it has controls the core interprets or renders for. A device that only reports values or occurrences (a barcode scanner, a keyboard, a sensor) is a connection with state and events. Both trigger flows the same way.

Shapes use TypeScript notation. The source of truth is the Rust types in `covalence-core` ([overview: project entities](overview.md#project-entities-are-rust-types)).

**Markers.** Unmarked rules are implemented (M5 Phases 1–4). **(Now)** is the reshaping in [M5 Phase 5](../milestones/m5-surfaces-and-bindings.md), which replaced pages, the `k`-grid, `input.raw` and the `gesture` shapes; it's implemented too. **(M6)** to **(M9)** name the milestone that implements a rule: [M6](../milestones/m6-primitives.md) (sources, selectors, event triggers, core actions), [M7](../milestones/m7-surfaces-from-plugins.md) (hardware surfaces), [M8](../milestones/m8-dataflow.md) (helpers as reducers, layer resolution), [M9](../milestones/m9-composition.md) (templates and components). **(Later)** has no milestone yet.

**(M9) Layers become components.** M5 and M8 built `layer` and `control_binding` entities. This spec describes them as components shown on surfaces ([composition §7.4](composition.md#74-what-components-replace) lists each rename). The state, events, triggers and navigation already use the new names (`component`, `shown`, `startComponent`); the entities, `/layers` and `/control-bindings` are next. The rules of resolution, cascading, fit and navigation are what M8 built.

Values that may follow state are `Source`s, and sets of controls are `Selector`s, as defined in the [dataflow spec](dataflow.md) §1–§2.

---

## 1. Entities

```ts
interface Surface {               // project entity `surface`
  id: string;                     // ULID
  name: string;                   // "FOH deck"
  slug: string;                   // state key: surface.<slug>
  model: string;                  // "virtual.deck15"; (M7) hardware models from their plugin
  serial?: string;                // (M7) matches a hardware device on attach
  startComponent?: string;        // shown component in the `main` group on first attach (M5: `startPage`; M8: `startLayer`)
  provider?: string;              // (M7) the connection whose device this is
  virtual?: {                     // (M8) a virtual deck's encoders and faders (§8, §11.4)
    encoders?: number;            // 0–4, default 0
    faders?: number;              // 0–4, default 0
    motor?: boolean;              // whether the faders' level moves them; default true
  };
}

// A shown component: composition §7, with `show` (composition §7.3):
//   on?: Selector<SurfaceId>;      (M8) surfaces or surface groups it may appear on
//   when?: When;                   (M8) §5.3; default { nav: "main" }
//   level?: "under" | "normal" | "over";   (M8) §5.3; default "normal"
//   rank?: Source<number>;         (M8) within the level; default 0
//   cascade?: "replace" | "through";  (M8) §5.2.1; default "replace"
//   parent?: string;               (Later) the tree preset, §5.3
// and `controls: ControlSpec[]`, the surface controls it covers (M5: `layout`; (Later) a Selector<ControlId>).

interface SurfaceGroup {          // (Later) project entity `surface_group`
  id: string; name: string; slug: string;
  members: { surface: string; offset: { x: number; y: number } }[];
}

// A component's bindings are in the bindings spec, §1, and its children in composition §8.
```

- **Slugs** follow the state segment rule (`[A-Za-z_][A-Za-z0-9_]*`) and are unique per entity type, as for flows.
- **A surface entity exists before its device attaches.** The virtual surface is created in the UI. **(M7)** Hardware surfaces are created on first attach from a plugin, matched by `model` and `serial` after that ([plugin protocol §13.6](plugin-protocol.md#136-device-drivers-are-plugins-the-core-performs-hardware-io)).
- **A shown component is independent of surfaces.** The same component can control controls on several surfaces at once, and its bindings are evaluated once for all of them.
- **(Now) A shown component lists its controls,** by the surface's control IDs: unique, at most 256 (composition §7.1). The editor's "New page" for a surface copies that surface's controls with their capabilities.
- **(Later) A surface group** places several surfaces on one canvas, each offset into the group's coordinates, so they work as one wide device. A component `on` a group covers its controls by region, or by qualified ID `<surface slug>/<control id>`.
- **(M7) `provider`** ties a surface to the connection whose device it is. It's set by the attach, and a surface whose provider is deleted is kept, detached.

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
- **(Now) Outputs.** `image`, and **(M8)** `level` through the bindings' `value` property ([§11.4](#114-level-outputs-echoes-gesture-timing-and-per-surface-helpers)). **(Later)** `led` and `text` through the bindings' `led` and `text` properties, and surface-wide brightness.
- **(M6) Controls are named by selectors.** Wherever one or many controls are named (a shown component's `controls`, a repeated child's `over`, an event source), the field is a `Selector<ControlId>` (dataflow §2): an ID, a list, `{region: Rect}` for every control whose `at` lies inside, `{near: "left" | "right" | "above" | "below", of}` for the nearest control in that direction, `{has: {inputs?, outputs?}}` by capability, `{expr}`, and `any`, `all` and `not`. M6 implements the selector in the core and IDs and lists in each field; the other forms reach each field as it needs them.

---

## 3. State

| Path | Value | Written by |
|---|---|---|
| `surface.<slug>.connected` | `boolean` | The core, on attach and detach |
| `surface.<slug>.model` | `string` | The core, on attach |
| `surface.<slug>.current.<group>` | component slug, or `null` | **(Now)** The core and the API: the current component of a navigation group (§5.3). `main` replaces M5's `surface.<slug>.page`, and is set on attach as `page` was. **(M7)** `main` is kept across restarts (stored, not replayed): a surface attaches showing the component it last showed, if that still fits, and its `startComponent` only if it has never shown one. **(M8)** Other groups (`popover`) are navigated the same way and start empty after a restart |
| `surface.<slug>.shown` | component slugs, highest first | **(M8)** The core: the active shown components in resolution order (§5.2), while attached (M8: `layers`) |
| `surface.<slug>.controls.<id>.component` | component slug, or `null` | **(M8)** The core: the shown component controlling the control (§5.2), while attached. With `through` components (§5.2.1), the one that has its input (M8: `layer`) |
| `surface.<slug>.controls.<id>.look` | `{[prop]: component slug}` | **(M8)** The core, while attached: which shown component each property comes from, for the properties some component binds (§5.2.1). Journaled with `component` |
| `surface.<slug>.controls.<id>.pressed` | `boolean` | **(Now)** The device: `momentary` input |
| `surface.<slug>.controls.<id>.velocity` | `number`, 0 to 1 | **(Now)** The device: the last `down`, if the capability has `velocity` |
| `surface.<slug>.controls.<id>.detents` | integer | **(Now)** The device: the running total of `relative` input since the attach |
| `surface.<slug>.controls.<id>.value` | `number`, or `[x, y]` | **(Now)** The device: `absolute` input |
| `surface.<slug>.controls.<id>.touched` | `boolean` | **(Now)** The device: `touch` input |
| `surface.<slug>.controls.<id>.requested.<output>` | the output's value | **(M8)** The core: what it last sent to a `led`, `level` or `text` output (§4.4), with `cause` → the latest change among what the binding read. `requested.level` is implemented |
| `surface.<slug>.controls.<id>.helpers.<name>` | a helper's value | **(M8)** A helper with `scope: "surface"` (§4.3) |
| `component.<slug>.controls.<id>.<name>` | a helper's value | **(M8)** A helper on a shown component's binding (§4.3) (M8: `layer.<slug>…`) |
| `component.<slug>.params.<name>` | a parameter | **(M9)** An instance's or a template's parameter (composition §1) |
| `component.<slug>.flows.<flow>` | owned flow status | **(M9)** A flow the component owns, per instance (composition §2) |
| `component.<slug>.items[<key>]` | a per-item instance | **(M9)** A shown component with `forEach`: each item's params, cells and flows (composition §4) |
| `component.<slug>.parts.<child>` | a child | **(M9)** Its params, its helpers' cells, its children and its flows' status (composition §8) |

- `surface` and `component` are reserved roots: a connection alias can't be either (state spec §1.1). **(M9)** `component` replaces the `layer` root, which replaced M5's `page`.
- **(M6) The `core` connection's actions name a surface by ID,** in their `surface` parameter: `navigate`, `inject` ([core actions §2](core-actions.md)).
- Surface state is held while the surface is detached, with `connected: false`, so a binding that shows a surface's current page keeps working. Input state is the exception (§4.1).
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

- **No lost edges.** Each `down`, `up`, `turn` and `touch` is applied in a scheduler turn of its own: the surface waits until `state` triggers and `waitUntil` steps have evaluated one change before it applies the next, for up to 50 ms. A tap shorter than a turn still gives two changes, and `state` triggers see each one. Bindings don't hold input back: they draw the latest state, since a key image can't show a change shorter than a frame. `value` input may be coalesced within a turn, with the latest value winning (state spec §1.5).
- **Device time.** `deviceTs` is the host's `ts`: monotonic per surface session, and never compared with the core's clock. Gesture timing uses it (§4.2). Ordering is by `seq`.
- **Volatile.**
  - **On attach,** input state is reset: every control released, `detents` at 0, and `value` absent until the device sends one.
  - **On detach,** every pressed control is released and every touched one untouched, with `cause` → the `state.patch` that set `connected: false`.
  - **After a restart,** input state isn't restored.
  - **After a failover,** the new active node starts with everything released ([state spec §7.4](state-and-event-log.md#74-redundancy-and-failover-in-three-levels-designed-not-built)).
- **Read-only.** Only the device writes input state. A `set` step only writes `local` and `vars` (flows §8.3), so flows inject input instead (§4.5).
- Input from a control the surface didn't declare is ignored and not journaled.

### 4.2 Events

**(Now)** Each input patch also gives **edge events**, journaled as `input.event` with `cause` → the patch, replacing M5's `input.gesture`. **(M6)** They're addressed at the control, `surface.<slug>.controls.<id>:press`, and journaled as `event` with `surface`, `component` and `control` as context ([dataflow §10.2](dataflow.md#102-events-are-addressed-by-state-path), dataflow §3). `delta` and `value` are in the payload.
- **Each event carries `component`:** the shown component that controlled the control when the input arrived (§5.2), and **(M9)** `instance`, the chain down to the child whose control it is (composition §7.2).
- **A timed `long` or `hold`** has `cause` → the patch for the `down` it follows, and carries the component that controlled the control at that `down`.

| Event | Fires | Notes |
|---|---|---|
| `press` | `pressed` goes true | Straight away, never delayed by the double-press window |
| `release` | `pressed` goes false | Carries the component of its `press`, even if another has taken the control since |
| `double` | `pressed` goes true within `doubleWindow` of it last going false | Fires **as well as** `press`. A flow that should act only on a double listens for `double` alone |
| `long` | Once, when a control has been pressed for `longAfter` | |
| `hold` | At `longAfter`, then every `holdEvery` while the control stays pressed | `delta` is the repeat count, from 1 |
| `turn` | `detents` changes | `delta` is the change |
| `change` | `value` changes | `value` is the new value. **(M8)** Not for echoes (§4.4) |
| `touch`, `untouch` | `touched` goes true or false | |

| Setting | Default |
|---|---|
| `doubleWindow` | 300 ms |
| `longAfter` | 500 ms |
| `holdEvery` | 150 ms |

- **Timing uses the device's `ts`**, so jitter between the device and the core doesn't change what a gesture is.
- **`long` and `hold` need a timer,** because no input arrives while a key is held. The core starts one when the `down` arrives. If the `up` arrives first and its `ts` shows the key was held for `longAfter` or more (late delivery), `long` fires before `release`. A `hold` repeat that would fire after the `up`'s `ts` doesn't fire.
- **Component-relative input is events, not state.** Two surfaces can each show one component and each hold its `k0`, so `component.<slug>` has no `pressed`. An event trigger on `{component, controls}` is matched per surface (§7).
- **(M8)** The timing settings are configurable per control with `gestures` on its control binding (bindings §1). It configures events rather than computing a value, so it's a setting, not a helper. Each is a `Source`: a duration (`400ms`), a number of milliseconds, a `{ref}` or an `{expr}`. They're read when needed, from the binding that has the control's input on that surface: `longAfter` and `holdEvery` when it goes down, `doubleWindow` at the next press. A press keeps the timing it started with, even if the controlling component changes during it. A failure, or a value that isn't a duration, gives that setting its default and is reported as the binding's problem, `gestures.<setting>` ([§11.4](#114-level-outputs-echoes-gesture-timing-and-per-surface-helpers)).

### 4.3 Helpers (M8)

A **helper** builds on a control's input to give derived state, events, or both. **A helper is a reducer** (dataflow §4.2) on the control's `control_binding` (bindings §5), whose `on` and `input` default to that control ([dataflow §10.3](dataflow.md#103-sources-transforms-and-sinks-helpers-are-transforms)). The same reducers work anywhere else as a variable's `value`, so a GPIO contact or an OSC fader gets them too. Input state and events stay available for anything else.

| Helper | Reads | Gives |
|---|---|---|
| `toggle` | `press` | a boolean, flipped on each `press` |
| `counter` | `press` | a number, stepped on each `press` |
| `position` | `detents`, `value` | a number in `[min, max]`: step, wrap, acceleration, `follow` |
| `velocity` | `detents`, `value` | a rate in units per second, smoothed, decaying to 0 when the control stops |
| `select` | `detents` | an integer index, or an item, over a count or a list |
| `absolute` | `value` | the value through a curve, with pickup and `follow` |
| `rate` | `value` | −1 to 1 from the control's centre, with a deadband |
| `latch` | events | set by one event, reset by another |
| `script` | everything above | whatever its expression returns: values and events |

- **Each helper is a cell** (dataflow §5), written under `as` (the helper's name by default):
  - at `<component's home>.controls.<id>.<name>`, shared by every surface the component is on: `component.<slug>.controls.<id>.<name>` for a shown component, under `parts` for a child (composition §7.1)
  - with `scope: "surface"`, at `surface.<slug>.controls.<id>.helpers.<name>` instead, one value per surface. **(M8)** Its cell exists from the first time the surface shows the component until the binding or the surface is deleted, so a surface's value survives navigating away and back, and reattaching. Its `on` defaults to `{surface, controls: <id>}`, and only input that arrived while the helper's component had the control's input steps it ([§11.4](#114-level-outputs-echoes-gesture-timing-and-per-surface-helpers))
- **`to`** sends the value on to a variable or an action parameter, as for any cell (dataflow §7).
- **Events** from a helper (a `toggle`'s `on` and `off`, a `select`'s `select`, a `script`'s own) happen at its cell, `component.<slug>.controls.<id>.<name>:on`, and match event triggers on `{component, controls}` with the helper's event name. A surface-scoped helper's events also carry its surface, so they match `{surface, controls}` too.
- **Options are `Source`s,** so a step, a bound or a list can follow state.
- **`follow`, `holdoff` and `pickup`** are as in dataflow §4.3.
- **Helpers run in the core,** as sandboxed transform definitions (dataflow §4.4). A plugin can suggest defaults for a control (`ControlDef.suggest`, plugin protocol §8), and **(M9)** ship transforms of its own, which also run in the core.

### 4.4 Reported and requested values (M8)

- **A two-way control has two values:**
  - what the device reports (`value`, `pressed` …)
  - what the core last sent (`requested.level`, `requested.led`, `requested.text`)

  Bindings and expressions can read both.
- **Echoes.** Input is an **echo** when the host marks it `echo: true`: the device moving itself, such as a motor fader following what it was sent. Recognising its own motor's movement is the job of the plugin that drives the device, by touch or whatever the device offers ([§11.4](#114-level-outputs-echoes-gesture-timing-and-per-surface-helpers)). An echo updates input state and gives no events.
- The loop guard (flows §8.6) is the backstop for loops the echo rule misses.
- **Two kinds of loop, two rules.** The echo rule is for a control reporting back what the core just sent it: a motor fader moving to a `value` binding, an encoder ring. A device's own state coming back after a sink's action (a mixer's volume, a PTZ's position while it moves) is handled by the sink and `follow` (dataflow §4.3, §7): a sink doesn't fire on a change that descends from its own action, and `follow` pauses while the control is touched and for `holdoff` after the last input.
- **Why it's in M8.** M8 brings faders, encoders and `follow`. A separate motorised controller would fight its own feedback the first time both directions are bound, unless its plugin marks the motor's movement. The core's half is in M8; each such plugin brings its own rule.

### 4.5 Injecting input (M6)

- **The `core` action `inject`** on a surface target puts input on a control as if the device had sent it: `down`, `up`, a `turn`, a `value` or a `touch` ([core actions §3.1](core-actions.md)). It goes through §4.1 and §4.2 unchanged, with the flow run (or the UI user) as origin.
- This covers "press button X" macros, virtual buttons in the UI, and testing a page without hardware.
- Injected input is `internal` (core actions §4), so it works in simulate mode.

---

## 5. Showing components

### 5.1 What a shown component is

A component with `show` (composition §7.3) is shown on surfaces. One that covers every control of a surface is a page, as M5 had. One that covers three keys is a shift layer, a popover or a section. A shown component with no `when` is in the `main` navigation group, so a project without the fields of §5.2 behaves as M5 did: the surface's current page controls the controls it covers. Its children (composition §8) are on wherever it is; only shown components take part in this section's rules.

### 5.2 Who controls a control (M8)

For each surface and each control, the **controlling component** is chosen from the shown components that:
- cover the control (`controls`, evaluated for this surface)
- fit the surface (§5.4). One that doesn't is never a candidate there, whatever its `when`
- may appear on this surface (`on`)
- are active (`when`, evaluated for this surface)

The highest `level` wins, then the highest `rank`. A tie goes to the one that became active most recently, then to the oldest (by ID). A control none covers is blank and ignores input.

- **The controlling component sets the control's look and gets its input,** through its binding on the control or the child that has it. Nothing is resolved per property, and nothing is inherited from components underneath. An input belongs to the component controlling its control as it arrives, before the input itself can change that, and a release to its press's component. That's the `component` in its events and in `{component, controls}` triggers, and whose `gestures` time it.
- **`rank`** is clamped to 0–99. One that isn't a number, or fails, counts as 0 and is reported as the component's problem, as are failing `on` and `when` sources (a failing `when` is inactive).
- **A shown component becomes active** when its `when` goes from falsy to truthy, or when navigation selects it (§5.3). Becoming active again moves it to the top of its level and rank.
- **Activation order is state:**
  - `surface.<slug>.shown` lists the active shown components, highest first.
  - `surface.<slug>.controls.<id>.component` names the controlling component.

  Both are journaled as one `state.patch` per surface and turn, with origin `{core: "dataflow"}` and `cause` → the latest change among what the surface's `show` fields read, so `why` explains who controls a key: "k3 is Shift's because k15 was pressed". After a restart, the order is rebuilt from current state, with ties going to the oldest. While the core runs, a surface that detaches and attaches again keeps its order.
- **`self`.** In a shown component's `controls` and `show` sources, `self` is the surface being resolved: `self.controls.k15.pressed`, `self.current.main` (dataflow §8).
- **Guards.** `show` sources can't read resolution results (`surface.<slug>.shown`, `.controls.<id>.component`, and the same through `self`); saving such a component fails with `resolutionCycle`. The check reads refs and the dotted names in expressions, so a path computed at run time isn't caught. Resolution runs once per scheduler turn, after cells have settled (dataflow §5.3): it's the state turn's last step, and its results are written in the same turn. A cell that reads them sees them from the next turn.
- **Armed components.** A candidate whose `when` reads the surface's input state (`self.controls.*`, which includes surface-scoped helpers) has its bindings evaluated ahead of time while inactive, so that holding shift redraws within the 16 ms budget. A latching shift reads a component-scoped `toggle`, isn't armed, and is evaluated when it becomes active.
- **The editor shows a control's candidates:** "k3: Shift (over, while k15 is held), otherwise Cameras (main)". It can simulate a state ("hold k15") to show what would change.
- **Ties are flagged.** Two shown components that can be active at once, at the same level and rank, on the same control, get an `ambiguousOrder` warning in the editor. Recency still breaks ties at run time, since popovers in different navigation groups rely on it, but a project shouldn't depend on it by accident.

### 5.2.1 Cascading through (M8)

A `replace` component, the default, works as above: it sets the whole look of the controls it covers and gets their input. One with `cascade: "through"` sets only what it binds, and everything else comes from the components below.

- **Each property is resolved on its own.** For each control and each `ControlProp` (bindings §1), the property comes from the highest shown component, by the order of §5.2, that either binds it or is `replace`. A `replace` component that doesn't bind a property leaves it blank, so a page never shows properties from components under it.
- **Input is resolved the same way, as one more property.** A binding's `input` (bindings §1) is `claim`, `pass` or `block`. The highest component whose binding on the control claims or blocks it, or the highest `replace` component, gets the control's input, and that's the component in `.controls.<id>.component`, in events' `component` context and in `{component, controls}` triggers. `block` takes the input and does nothing with it.
- **Defaults.** In a `replace` component, input is `claim`. In a `through` one it's `pass`. The editor sets `claim` when you add an action, a helper or a flow trigger to a key in a `through` component, so it's always stored, never inferred. A child's bindings take the default of the shown component they're in.
- **Helpers and gestures belong to the input.** They run on the component that has the control's input.
- **`why` explains each property.** `.controls.<id>.look` names the source component per bound property and is journaled, so "why is k3 red?" answers "background from Cameras, label from Shift".
- **Rendering doesn't change.** The renderer draws from properties, taken from more than one component, and armed components (§5.2) still let a `through` shift redraw within 16 ms.

Some examples:

| Component | Cascade | Binds | Effect |
|---|---|---|---|
| Shift | `through`, `level: over` | `label`; `input: claim` | Keys show their alternate action and keep the page's colour and tally |
| Lock | `through`, `level: over`, `when: {while: vars.locked}` | **(Later)** `icon` (a padlock), or a `label`; `input: block` | Every key keeps its live look and ignores presses |
| Rehearsal | `through`, `level: over`, `when: {while: sys.simulate}` | `background` | Keys go grey while simulating; labels, input and everything else are unchanged |
| Confidence | `through`, `level: over` | `text` on the deck's display | Shows a clock and status without taking any keys |

**Rejected:** reading the look underneath (`below.background`), which open question 11 asked about. It would make a binding depend on resolution, which the `resolutionCycle` guard forbids, and it only helps a component that sets the property anyway.

### 5.3 Presets

Presets fill in `show` and `controls` for common structures, and the editor names a shown component by its preset: a page, a shift layer, a popover. As with structured bindings ([bindings §9.2](bindings.md#92-the-structured-kinds-are-evaluated-natively)), they're stored as presets, evaluated natively, and shown with the expression they compile to. "Edit as expression" is one-way.

```ts
type When =
  | { nav: string }               // a navigation group: active while it's the group's current component
  | { always: true }
  | { while: Source<boolean> };   // active while truthy: a ref, a `compare`, an expression…
```

| Preset | Fields | Compiles to |
|---|---|---|
| **Page** (navigation group, the default) | `when: {nav: "main"}` | `self.current.main == me`. Navigation sets `surface.<slug>.current.<group>`; folders are pages in one group with navigation keys between them |
| **Levels** | `level: under \| normal \| over` | `rank` bands of 0, 100 and 200, with `rank` (0 to 99) inside the band |
| **Shift** | `level: over`, `when: {while: {expr: "self.controls.k15.pressed"}}` | Active while the key is held. A latching shift reads a `toggle` helper's value |
| **Overlay** | `level: over`, `cascade: through` | Changes some properties of the keys under it: a lock, rehearsal grey, a confidence display (§5.2.1) |
| **Popover** | `level: over`, `when: {nav: "popover"}` | Opened by navigating the `popover` group to it, dismissed by navigating it to `null` |
| **Tree** | `parent: <component>` | **(Later)** `when` is combined with the parent's (both must be active). `rank` follows position among siblings, so a later sibling covers an earlier one. It writes `when` and `rank`; resolution doesn't walk it ([composition §11.8](composition.md#118-one-component-for-keys-strips-and-pages)) |
| **Section** | `controls: {region}`, `when: {nav: "<section>"}` | A region of the deck with its own current component, as in Bitfocus Buttons |
| **Always on** | `when: {always: true}`, usually `level: under` | A strip that shows wherever nothing else covers it |

- **Navigation** is an API call (§9) and **(M6)** the `core` action `navigate` ([core actions §3.1](core-actions.md)). Each group keeps a short history for **(Later)** `back`.
- **(M9) Parameters** (composition §1) are readable as `params` by the component's own bindings, helpers, `show` fields and owned flows. The tree preset doesn't pass them to descendants: a component that needs one declares its own and binds it ([composition §11.1](composition.md#111-reuse-is-by-parameters-never-by-copying-or-overriding)).

### 5.4 Fit

- **(Now) A shown component fits a surface** when every one of its listed controls exists on the surface with at least the inputs and outputs it declares for it. The surface may have controls it doesn't.
- One that doesn't fit can't be navigated to on that surface (`doesNotFit`, §9), which replaces M5's equal-layout rule.
- **(Later)** One whose `controls` is a selector always fits. It covers whatever controls the selector finds there.
- **(Later) Placement per surface** maps a component's controls onto a surface of a different shape.

---

## 6. Lists over controls (M9)

Control groups, planned here, are replaced by a **child with `forEach`** ([composition §8.2](composition.md#82-one-child-per-item)): a live list spread over a set of a component's controls, one item per control, with `item`, `index` and `key` in its params and in `trigger`, and `overflow: "scroll"` keeping an offset at `<home>.parts.<slug>.offset`. Nothing is saved per item.

---

## 7. Triggers

Flows can start from a control in two ways (flows §1.1). **A control's binding holds its look and its helpers. What happens when it's used lives in flows**, and the key editor shows every flow that references a control (flows §6).

**Event triggers** are the quick way in, for events. **(M6)** A control's events are events like any other ([dataflow §10.2](dataflow.md#102-events-are-addressed-by-state-path)), so the `control` trigger of M5 Phase 5 becomes an `event` trigger whose source names controls (dataflow §3.1):

```ts
{ kind: "event", from: EventSource, event: string | string[], filter?: Expr }

// The control forms of EventSource:
  | { component: string; controls?: Selector<ControlId> }  // that component's controls, wherever it's shown or used (M8: `layer`)
  | { surface: string; controls?: Selector<ControlId> }    // (Later) whatever component controls them
  | { controls: Selector<ControlId> }                      // (M9) in a flow a component owns: its controls (composition §2)
  | { path: Selector<Ref> }                                // any control by path: {match: "surface.*.controls.*"}
```

- **Migration (M6).** `{kind: "control", control: {layer, control}, event}` becomes `{kind: "event", from: {layer, controls: control}, event}`. Stored flows are migrated when M6 loads them. **(M9)** `{layer}` becomes `{component}`, and `{slot}` and `{group}` aren't needed (composition §7.4).
- `event` is one of the events in §4.2, or a helper's. A `{component}` source matches **on any surface where that component had the control** when the input arrived: as the shown component, or **(M9)** as a child of one, so one trigger on "Tally key" covers every tally key. `component` is an ID; the editor shows its name.
- **`controls` is a selector** over the component's own controls, so one trigger covers a region, every encoder (`{has: {inputs: ["relative"]}}`) or a list. Absent, it's every control of the component.
- **The `trigger` payload** is the event payload of dataflow §3.1: `{path, event, payload, surface, component?, control, instance?}`, with `delta` and `value` in `payload`, plus **(M9)** `item`, `index` and `key` for a control of a repeated child. `surface` and `component` are entity IDs.
- `trigger.matched` has `cause` → the control's `event` entry and the surface as its origin, so `why` on an action reaches the input's patch.
- **Validation:**
  - The component must exist (`unknownComponent`; M8: `unknownLayer`), and every control the selector names must be one of its listed controls (`unknownControl`).
  - An event none of the selected controls' inputs can give (a `turn` on keys) is `unsupported`.

**`state` triggers** work on input state as they do on any state, for conditions and combinations:
- A press only while armed: `when: surface.deck.controls.k0.pressed && vars.armed`, `edge: rising`.
- A chord: `when: surface.deck.controls.k3.pressed && surface.deck.controls.k4.pressed`, `edge: rising`.
- A helper's value: `when: component.cams.controls.dial1.position > 0.5`, `edge: rising`.
- **(M6) Pattern `state` triggers** match many paths at once: `match: "surface.*.controls.*.pressed"`, with the payload `{path, captures, value, previous, ts}` (flows §1.1). They work on any connection's state the same way.

A shift key is usually a shown component (§5.3) rather than a condition, so the deck shows what the shifted keys do and the editor has a key to select.

---

## 8. The web surface

The browser's virtual deck is a surface like any other ([§11.1](#111-surfaces-are-core-entities-independent-of-transport)). It connects to `GET /surface/{id}`, a WebSocket that carries the plugin protocol's `surface.*` notifications (plugin protocol §8) as JSON text frames:

```json
{ "method": "surface.attach", "params": { "surface": "<slug>", "model": "virtual.deck15", "name": "…", "controls": [
  { "id": "k0", "at": { "x": 0, "y": 0 }, "inputs": [{ "kind": "momentary" }],
    "outputs": [{ "kind": "image", "w": 144, "h": 144, "format": "png" }] }, … ] } }
{ "method": "surface.input",  "params": { "surface": "<slug>", "control": "k3", "kind": "down", "ts": 1234.5 } }
{ "method": "surface.draw",   "params": { "surface": "<slug>", "control": "k3", "format": "png", "image": "<base64>" } }
{ "method": "surface.level",  "params": { "surface": "<slug>", "control": "f0", "value": 0.7 } }
```

- **(Now)** The virtual deck declares 15 controls `k0`…`k14`, each `momentary` with a 144 × 144 PNG `image`, placed 5 × 3 with `at`. The attach has no `layout`.
- **(M8)** The surface's `virtual` adds, per [§11.4](#114-level-outputs-echoes-gesture-timing-and-per-surface-helpers):
  - encoders `e0`…`e3` in a row beneath the keys (`y` 3), each `relative` + `momentary` with a `level` output for its ring
  - faders `f0`…`f3` to the right of the keys (`x` 5…8, three cells tall), each `absolute` + `touch` with a `level` output, `motor` as configured

  A motorised virtual fader glides to each level it's sent. It reports `value` only while it's dragged, with `touch` 1 on pointer down and 0 on pointer up, so it never echoes. Changing `virtual` makes the browser attach again with the new controls. `covalence_core::surface::virtual_controls` builds the same list for tests.
- **One surface per socket.** The socket's `{id}` must name an existing surface entity, or the upgrade is refused with `404`. The first message must be `surface.attach` within 10 s, and it and every `surface.input` must use the surface's slug.
- **Errors close the socket** with code 1008 (policy) and the reason as the close frame's text:
  - a message that isn't a notification
  - an attach the core refuses (`cannot attach: …`)
  - another surface's slug
  - a method other than `surface.input` and `surface.detach` after the attach
  - **(M8)** `virtual` on a surface whose model isn't `virtual.deck15`, or more than 4 encoders or faders, is `invalid` when the surface is saved
- **A second attach for the same surface replaces the first,** whose socket is closed with 1000 and `attached elsewhere`. Two browser tabs can't drive one surface. Deleting the surface closes it with 1000 and `surface deleted`; shutting down, with 1001.
- **`ts`** is the browser's `performance.now()` in milliseconds.
- **Images are base64 PNG** at the size each control's `image` output declares. The virtual deck declares 144 × 144 so keys stay sharp on high-density screens.
- **The socket closing is a detach.**

---

## 9. HTTP API

Surfaces and components have the same routes and answers as flows (flow spec §9): `POST` creates and assigns the ID, `PUT /{id}` replaces, each answers `{surface | component, warnings}` or `422 {errors, warnings}`, and a missing slug is derived from the name. **(M9)** `/components` replaces M8's `/layers` and `/control-bindings`: a binding is saved with its component.

| Method | Path | |
|---|---|---|
| `GET` | `/surfaces`, `/components` | List. `/components?shown=true` lists shown components only |
| `POST` | `/surfaces`, `/components` | Create, validated like flows |
| `GET`, `PUT`, `DELETE` | `/surfaces/{id}`, `/components/{id}` | Read, replace, delete. Deleting a surface detaches it and deletes its state. Deleting a component deletes its bindings and clears it as any surface's `startComponent`; one used as a child, or with instances, can't be deleted (`409`, composition §5); a flow whose event trigger names it gets `unknownComponent` when it's next saved |
| `GET` | `/surfaces/{id}/controls` | **(Now)** The controls the surface declared when it last attached, for "New page" in the editor. `404` if it has never attached |
| `POST` | `/surfaces/{id}/current` `{group?: "main", component: <id> \| null}` | **(Now)** Navigates a group and answers `{surface, group, component}` once the surface has been sent its controls' outputs. **(M8)** Any group, named like an identifier (`422 {code: "invalid"}` otherwise); only `main` before. Journaled as a `state.patch` on `surface.<slug>.current.<group>`, origin the user. `422 {error, code: "unknownComponent"}` (also for a component with no `show`), or `409 {error, code: "doesNotFit", controls}` listing the controls the surface lacks or can't support. Replaces M5's `POST /surfaces/{id}/page` and `layoutMismatch` |
| `GET` | `/components/problems` | **(M8)** `[{component, field, message}]`: each `show` field (`on`, `when`, `rank`) or binding property (`["bindings", <id>, "props", <prop>]`) that failed when last evaluated (§5.2, bindings §3) |
| `POST` | `/components/preview` `{binding, size?}` | The editor's preview of one binding (bindings §2.2) |
| `GET` | `/surface/{id}` | The web surface WebSocket (§8) |

- **Validation:**
  - **A surface** needs a `name`, a `model` and a slug no other surface has (`badSlug`, `duplicateSlug`), and its `startComponent` must exist and be shown (`unknownComponent`).
  - **A component** needs a `name` and a unique slug. **(Now)** Its controls need unique IDs (`duplicateControl`), there are at most 256, and every bound control must be among them (`unknownControl`). Bindings and children follow composition §7.1 and §8.
  - **(M8)** `show` sources can't read resolution results (`resolutionCycle`).
  - **Bindings** are validated as in bindings §2.1.
- **Edits answer once drawn.** Saving or deleting a component, and replacing a surface, answer after surfaces showing the change have been sent their controls' outputs.

- **(M6)** Flows navigate and inject input with `core` actions ([core actions](core-actions.md)). `POST /surfaces/{id}/current` stays, for the UI, and does what `navigate` does.

---

## 10. Open questions

1. ~~Core-provided actions (navigate a group, back, set brightness, inject input) for flows: a reserved `core` target, or new step kinds?~~ Decided by [core actions §6.1](core-actions.md#61-the-core-is-a-connection): the core is a connection, and its actions name a surface by parameter.
2. ~~Mapping a page onto a surface of a different size~~: shown components list their controls and fit by capability, or cover a region (§5.4). Explicit placement per surface is Later.
3. Should hardware surfaces that attach for the first time get an entity automatically, or wait in an "unassigned" list until a user adds them?
4. Should surfaces declare shapes (a keypad, a mixer) that name controls by role, so one page can target any surface of a shape?
5. Should a component's control be able to ask for a capability range (an image of at least 72 × 72) rather than just a capability kind?
6. **Capabilities beyond the four inputs and four outputs:** pressure and aftertouch, more than two axes, multi-position switches, haptics, sounds. Should unknown kinds attach with their input written as raw state under the kind's name?
7. **Whole-surface canvases:** a Loupedeck or tablet screen with touch zones, drawn as one image rather than per control.
8. **Time-based output:** blinking and animation. Bindings follow state, not time.
9. **Sequences** ("A then B within 1 s"): these need recent history, not current state. A reducer (dataflow §4.2) that steps on both events and emits `matched` may be enough.
10. **Orbitals while the core is unreachable:** should an Orbital keep showing the last outputs, or run some feedback locally?
11. ~~**Reading the look underneath:** a shift that relabels a key has to set its colour too.~~ Decided in §5.2.1 and §11.3.2: `through` components cascade per property, with input as one more property. Reading the look underneath is rejected.

---

## 11. Decisions

Why surfaces and showing components are this way, and what was rejected. Keep these notes when the sections above change.

### 11.1 Surfaces are core entities, independent of transport
- **Decided:** `surface` and **(M9)** `component` (M5: `page`, M8: `layer`) are project entities, with slugs for their state keys as flows have. A surface entity exists before its device attaches. The core's surface module doesn't know its transport: a transport attaches a surface with its controls, feeds it input and receives outputs. The web surface is one transport, over its own WebSocket `/surface/{id}` carrying the §8 messages as JSON with base64 images, one socket per surface, the socket closing being the detach. The plugin host is another (plugin protocol §8).
- **Why:** the browser deck runs the same core path as a hardware deck (attach, controls, input with device timestamps, rendered images), so what it shows is what a deck would show. A press reaches the core in one hop, with no batching. Base64 over JSON costs a third more bytes, which doesn't matter for 15 keys on a LAN.
- **Rejected:**
  - *Surfaces over `/ws`:* `/ws` coalesces and batches for the editor, speaks a different protocol, and a kiosk deck needs none of its feed.
  - *The virtual deck in the plugin host:* a hop and a process for a device already talking to the core's own server.
  - *The current page as an entity field:* switching would be a project edit (revision, undo, conflicts) rather than live state.

### 11.2 Surfaces are two-way devices, and control input is state
- **Decided:** a surface has state (what the device reports, and what the core asked it to show), events derived from its input, and outputs. Controls list capabilities (§2) rather than a kind, have free-form IDs and a position `at`. Input is state at `surface.<slug>.controls.<id>` (§4.1), written by the device only, volatile (reset on attach, released on detach and on promotion). Every input patch also gives edge events, each naming the component that controlled the control (§4.2). State is for conditions and combinations (shift keys, chords, "while held"); events are for occurrences, and for anything relative to a layer.
- **Why:** Covalence should handle any control device, hardware or software, and devices aren't input or output: a Stream Deck has buttons and screens, a motor fader is moved by the user and by the core. M5's model was shaped around one Stream Deck (a `k`-grid, equal layouts, one page at a time, fixed meanings), and changing that before saved projects depended on it was cheap. Treating input as a separate stream would be two models for one idea, but not all input behaves like state: coalescing loses edges, a turn has no value, and timing has to come from the device. So `momentary`, `touch` and `relative` inputs each get a turn of their own, and events carry the device's time. Supporting a new device is a transport and a capability list.
- **Gestures:** `press` fires on `down` at once, and `double` fires as well as `press`, never instead of it; making `press` wait for the double window would cost 300 ms on every press. A flow that wants only double-presses listens for `double`. A separate `tap` was rejected as a gesture whose meaning depends on timing nobody sees; it can come if users ask.
- **Rejected:**
  - *Keeping the `k`-grid and mapping positions across sizes:* only keypads ever fit.
  - *Input as a stream of signals with interpreters:* two models for device data.
  - *Input as state only, with counters for occurrences:* events in disguise, and no use for component-relative input.
  - *Merging surfaces into connections:* a surface's `provider` connection gets most of the benefit.
  - *Value mappings as their own entity:* a continuous control could only drive an action parameter, and one control would need two entities. Helpers on the control's binding replaced them.
  - *Helpers in plugins:* inconsistent behaviour, no core timing guarantees, and `why` stopping at the plugin. Plugins may `suggest` defaults.

### 11.3 Shown components decide who controls each control
- **Decided:** a shown component (M8: a layer) covers controls, with `controls`, `on`, `when` and `rank`. For each surface and control, the active shown component with the highest rank that covers it controls it (§5.2), and it sets the whole look and gets the input; its children are on wherever it is, and never take part in resolution themselves ([composition §11.8](composition.md#118-one-component-for-keys-strips-and-pages)); activation order is state, so `why`, bindings and the editor can read who controls a key. Guards: those four fields can't read resolution results (`resolutionCycle`), resolution runs once per turn, and a component whose `when` reads input state is rendered ahead of time so a shift redraws within 16 ms. Presets (§5.3) cover the known structures and compile to expressions, like structured bindings. Behaviour lives in flows that reference a control, and the key editor is a view of them.
- **Why:** real shows need more than one page at a time: shift keys, mode keys, popovers, a folder in one region while the rest stays put, grids from a live list, several decks as one. Prior art solves each with a structure (Companion's flat pages, SKAARHOJ Reactor's layer tree, Bitfocus Buttons' sections on a canvas), and building any one into the core rules out the others. One rule over state covers all of them as presets, and who controls a key is always a function of current state, so it can be shown live and simulated. The cost: every shown component's `when` and `rank` is evaluated per surface, and shown components are less portable until interfaces or regions arrive.
- **Rejected:**
  - *A fixed stack of pages, or recency alone:* a stack can't say "the last mode turned on wins", and recency loses an always-on strip on the first navigation. Both are presets.
  - *A layer tree with switch nodes at run time* (Reactor): one structure among several, and moving a branch changes behaviour. The tree is a preset and an editor view.
  - *Sections on a canvas* (Buttons): a second concept beside shown components. A section is a shown component with a region and its own navigation group.
  - *Conditional bindings with CSS-like specificity:* the hardest to predict, with no structure for the editor to show.
  - *Generating bindings at run time for dynamic grids:* the core would edit the project. A child with `forEach` (§6) evaluates a live list instead.
  - *Layering per property* (a shift layer inheriting the colour underneath): wrong for navigation, and two rules for one control are hard to explain. `through` components (§5.2.1, §11.3.2) are the answer: per property, but only where a component asks for it.

### 11.3.1 Resolution runs in the state turn
- **Decided:** resolution is the state turn's last step, after cells settle, and writes `shown` and `.controls.<id>.component` in that turn. The binding worker draws from its results, and a surface's input reads the controlling component from them as it arrives. A component that doesn't fit a surface isn't a candidate there. Navigation groups other than `main` aren't kept across restarts. A surface-scoped helper's cells are added when its component becomes its `main` group's current component, attached or not, or becomes active on the surface, before that surface's next input.
- **Why:** "k15 down, then k3 down" has to give k3's press to the shift layer. Resolving in the binding worker, after the turn, would let the second input land before the first had been resolved, unless every input waited for drawing too. In the turn, the resolution is there before the input task's next write, and `why` links it to the change that caused it. A component that doesn't fit would be bound on controls the surface lacks, or with outputs it doesn't have. A popover left open across a restart is a surprise on a show day; `main` is the page the operator chose.
- **Rejected:**
  - *Resolving in the binding worker:* drawing waits on resolution anyway, but input would see the previous resolution.
  - *Resolving in the dataflow graph, as cells:* resolution reads every shown component's fields for every surface and orders them, which isn't one source with one home; and cells reading its results would make the `resolutionCycle` guard a graph cycle.
  - *Partly fitting shown components covering the controls they can:* a shift layer that loses one key to a smaller deck still takes the other two, silently.

### 11.3.2 Cascading through
- **Decided:** a component is `replace` (the default) or `through` (§5.2.1). Each of a control's properties, and its input, comes from the highest active component that binds it or is a `replace` component; a binding's `input` is `claim`, `pass` or `block`, defaulting by its component. Where each property comes from is state, `.controls.<id>.look`, beside `.controls.<id>.component`, which is the component with the input. The binding worker evaluates every component a control's look comes from, and puts the look together from them; a look from one component is that component's, as before. A binding that blocks input can't have helpers. The editor sets `claim` when a helper or a press flow is added to a key in a `through` component, and warns `ambiguousOrder` when another component that can be active at the same time ties on level and literal rank over the same control (two shown components in one navigation group can't be).
- **Why:** shift, lock, rehearsal and confidence overlays want the live look underneath with one thing changed. Without it, each one copies the tally rules of every page it covers. Keeping `replace` the default keeps pages and popovers as they were, and `look` keeps `why` answerable per property.
- **Rejected:**
  - *Reading the look underneath* (`below.background`): a binding would depend on resolution, which the `resolutionCycle` guard forbids.
  - *Input following the highest `through` component that binds anything:* a relabelling component would steal presses it doesn't handle. Input is claimed explicitly.
  - *Warning on every tie, navigation groups included:* every page of a deck ties with every other, and they're never active together.

### 11.4 Level outputs, echoes, gesture timing and per-surface helpers
- **Decided:**
  - **`value` drives a `level` output** (0 to 1) for motor faders, light rings and meters. The surface link carries outputs, images or levels, on one channel per surface, so there's one order and one rule for a slow device: a value replaced before it was sent isn't sent. What was sent is state at `requested.level`, with `cause` → the change that moved it.
  - **Echoes are marked by the device's plugin** (`echo: true` on `surface.input`), and the core's only rule is that a marked input updates state and gives no events. The plugin knows its hardware: for most motor faders "moving while untouched is the motor", as DAW controllers and lighting desks do it.
  - **The virtual surface is configurable** (`virtual: {encoders, faders, motor}`), so encoders and motor faders can be tested without hardware (§8).
  - **`gestures` timing** is per control binding, as sources read when needed, not followed: `longAfter` and `holdEvery` at the down, `doubleWindow` at the next press. A press keeps the timing it started with.
  - **Helpers with `scope: "surface"`** have one cell per surface, from the first time it shows the component until the binding or surface is deleted, so a deck's toggles survive navigating away and back.
- **Why echoes matter:** told to go to 70%, a motor fader reports 40%, 55%, 70% on the way, as if pushed. Taken as input that goes to the mixer, comes back and moves the fader again, and the fader fights itself. The loop only exists with a separate motorised controller; the other loops are handled by sinks not sending back their own change, `follow`'s holdoff, `pickup` and deltas.
- **Rejected:**
  - *Echoes by timing in the core* (a window and a tolerance): two numbers that can be wrong (a hand grabbing the fader just after it moved is ignored), a new kind of event and an opt-in.
  - *Touch decides, in the core:* one family of hardware's rule in the core, and the plugin still needed for faders without touch.
  - *No motor feedback, with `pickup`:* the controller wouldn't show the real level.
  - *A second fixed virtual console model:* every new mix of controls would need another.
  - *Gesture timing as literals only:* a "novice mode" with slower double-presses would mean editing every binding.
  - *Surface helpers that reset when their component leaves:* toggles forgetting themselves on every page change.
