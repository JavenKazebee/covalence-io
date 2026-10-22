# 0024 – Surfaces are two-way devices; control input is device state; layers decide who controls each control

- **Status:** Proposed
- **Date:** 2026-10-22
- **Amends:** [0022](0022-surfaces-pages-and-the-web-surface.md) (the `page` entity becomes `layer`; pages addressed as a `k0…kN` grid, equal layouts, `gesture` triggers, `input.raw` and `input.gesture`; the surface entity, transport-independent core module and web surface stand). Replaces value mappings as a separate entity ([bindings §5](../specs/bindings.md)).
- **Specs:** [surfaces](../specs/surfaces.md), [bindings](../specs/bindings.md), [flows](../specs/flows.md), [plugin protocol §8](../specs/plugin-protocol.md), [state and event log](../specs/state-and-event-log.md)
- **Related:** [ADR 0008](0008-redundancy-and-failover.md), [ADR 0010](0010-orbital-remote-nodes.md), [ADR 0019](0019-dynamic-action-targets-and-interfaces.md), [ADR 0023](0023-key-rendering-and-binding-evaluation.md)
- **Milestone:** [M5: Surfaces and bindings](../milestones/m5-surfaces-and-bindings.md), Phase 5 for the parts marked **Now**

## Context
- **Devices.** Covalence should handle any control device, hardware or software: Stream Decks, MIDI controllers, X-keys, GPIO boxes, motor-fader panels, OSC tablets, web panels, Orbitals.
  - Flows should be triggerable by anything.
  - One physical control should be able to mean different things. An encoder might be a position, a velocity, a selector or a pair of buttons.
  - Devices aren't either input or output. A Stream Deck has buttons and screens; a motor fader is moved by the user and by the core.
- **M5's surface model is shaped around a Stream Deck:**
  - A page is `{cols, rows}`, and its buttons must be `k0…kN` in row-major order.
  - A page fits only a surface with an equal layout.
  - A surface shows exactly one page at a time.
  - What an input means is fixed: buttons get gestures, and continuous controls get a separate `value_mapping` entity (bindings §5).
- **Input is state.** Connections already describe devices as state (current values) and events (occurrences). A key being pressed and a fader's position are state too. Treating them as a separate input stream means two models for one idea.
- **But not all input behaves like state.** Coalescing can lose edges, and an encoder's turn has no value. Timing has to come from the device, and some occurrences never change a value.
- **Real shows need more than one page at a time:**
  - shift keys
  - mode keys
  - popovers
  - a folder in one region of the deck while the rest stays put
  - grids generated from a live list, such as the selected camera's ISO values
  - several decks used as one
- **Prior art solves these with structures:**
  - Companion's flat pages and navigation keys
  - SKAARHOJ Reactor's layer tree, with conditions to activate layers
  - Bitfocus Buttons' positions and sections (shift, folder, popover) on a shared canvas
- **Each of those structures is a special case.** Building any one of them into the core would rule the others out.
- **Now is the cheap time.** M5 has just landed and no saved projects depend on its shapes. Changing them now costs little, and it gets more expensive with every project and every line of UI.

## Decision

### Surfaces are two-way devices
- **A surface is a device with inputs and outputs:**
  - **state:** what the device reports, and what the core has asked it to show
  - **events:** occurrences derived from its input
  - **outputs:** images, LEDs, levels, text, brightness
- **Surfaces stay their own entity and state root.** Rendering, gesture timing and layer resolution are core services on the input path, within the 5 ms and 16 ms budgets.
- **What makes a device a surface:** it has controls the core interprets or renders for. A device that only reports values or occurrences is a connection with state and events. Examples are a barcode scanner, a keyboard and a sensor. Both kinds trigger flows the same way.
- **Later:**
  - **`Surface.provider`:** the connection whose device this is.
  - **Surface interfaces** ([ADR 0019](0019-dynamic-action-targets-and-interfaces.md)).
  - **Surfaces over the plugin host and Orbitals.**
  - **Surface groups:** several surfaces placed on one canvas and used as one (Buttons' positions). Each member's `at` is offset into the group's coordinates. Layers can be shown on a group, and its controls are addressed as `<surface>/<control>`.

### Controls are typed by what they can do, and have a position
- **A `ControlDef` lists its capabilities** instead of a single `kind`:
  - inputs: `momentary` (with optional velocity), `relative`, `absolute` (one or two axes) and `touch`
  - outputs: `image`, `led`, `level` and `text`
- **IDs are free-form.** The `k0…kN` rule and the attach's `layout` are gone. M5's kinds become SDK presets.
- **Every control has a position, `at: {x, y, w?, h?}`,** in grid cells. A device without a physical layout, such as GPIO pins, is placed in a row by its SDK preset. Positions are what regions, neighbours, surface groups and the editor's drawing of the device are built on.
- **Now:** capabilities on the wire and in the core, with `at` required. Only the `image` output does anything.
- **Later:**
  - **`led`, `level` and `text` outputs, and brightness.**
  - **Region refs** (`{region: {x, y, w, h}}`) and **relative refs** (`{relative: "left" | "right" | "above" | "below", of}`), wherever a control can be named.

### Control input is device state
- **Each control's input is state** at `surface.<slug>.controls.<id>`: `pressed` and `velocity` (`momentary`), `detents` (a running total, `relative`), `value` (`absolute`) and `touched` (`touch`).
- **It's applied as `state.patch`,** with the surface as origin and the device's `ts`. This replaces `input.raw`.
- **No lost edges:** each `momentary`, `touch` and `relative` input gets a scheduler turn of its own. `absolute` input may coalesce.
- **Volatile:**
  - Reset on attach, and released on detach.
  - Not restored after a restart.
  - Released on a node that has just been promoted.
- **Read-only:** only the device writes it.
- **Now.**

### Events are derived from input state
- **Every input patch also gives edge events,** journaled as `input.event` with `cause` → the patch. The events are `press`, `release`, `double`, `long`, `hold`, `turn`, `change`, `touch` and `untouch`.
- **Each event names the layer that controlled the control** when the input arrived.
- **What each is for:**
  - State is for conditions and combinations: shift keys, chords, "while held".
  - Events are for occurrences, and for anything relative to a layer.
  - A layer shown on two surfaces gets events from each surface separately, and is never given combined input state.
- **Now:** this replaces `input.gesture`.

### Triggers
- **`control` triggers** are `{control: ControlRef, event}`. They replace `gesture` triggers.
- **A `ControlRef` can be:**
  - **Now:** `{layer, control}`, for that control on any surface where the layer controls it.
  - **Later:**
    - `{surface, control}`
    - `{group}`: any control in a control group, with `trigger.item`
    - region and relative refs
    - selectors
    - expressions
- **`state` triggers already work on input state.** **Later:** pattern `state` triggers (`match: "surface.*.controls.*.pressed"`) fire for whichever path matched.

### Helpers (Later)
- **A helper builds on a control's input state** and gives derived state, derived events, or both:
  - `gestures` timing settings
  - `toggle`
  - `position`
  - `velocity`
  - `select`
  - `absolute`
  - `rate`
  - `script`: sandboxed, with the binding thread's time limit
- **Helpers live on the control's binding.**
- **Options can be literals, refs or expressions.**
- **Values are state,** for the layer, for each surface, or in a variable.
- **`to: {action}` replaces value mappings.**
- **`follow` keeps a value in step with state.** It pauses while the control is touched, and for a short hold-off after input.
- **Plugins can suggest helper defaults** for a control, but can't run their own.

### Reported and requested values, and injected input (Later)
- **Two-way controls keep both values:** what the device reports, and what the core last sent (`requested.<output>`).
- **Echoes are marked,** either by the host or by a timing window, and give no events unless a trigger asks for them.
- **A core action injects input** as if the device had sent it. It goes through the same path, so state, events, triggers and `why` all see it.

### Layers decide who controls each control
- **The `page` entity is renamed `layer`.** A layer is a set of control bindings on the controls it covers. There's one concept: a "page", a shift layer, a popover, a folder and a section are all layers, configured differently.
- **The core is computed from state.** A layer has four parts, and each can be a literal or an expression:

  | Field | Meaning |
  |---|---|
  | `controls` | The controls the layer covers: a list (Now), a region, or an expression (Later) |
  | `on` | The surfaces or surface groups it may appear on (Later; any surface it fits if absent) |
  | `when` | Whether it's active, evaluated for each surface, with `self` meaning that surface (Later) |
  | `rank` | A number; higher wins (Later; default 0) |

- **The resolution rule:** for each surface and control, the controlling layer is the active layer that covers the control and has the highest rank. Ties go to the most recently activated, then to the oldest layer (by ID). That layer sets the control's look and gets its input. Nothing is resolved per property, and nothing is inherited from layers underneath.
- **Activation order is state.** `surface.<slug>.layers` holds the active layers in resolution order, journaled. `surface.<slug>.controls.<id>.layer` names the controlling layer, so `why`, bindings and the editor can read who controls a key. After a restart the order is rebuilt from current state, with ties going to the oldest layer.
- **Guards:**
  - A layer's `controls`, `on`, `when` and `rank` can't read resolution results (`.layers`, `.controls.<id>.layer`). If they do, saving fails with `resolutionCycle`.
  - Resolution runs once per scheduler turn.
  - A layer whose `when` reads input state is evaluated and rendered ahead of time while inactive, so a shift redraws within 16 ms.
- **Presets cover the common structures.** Like structured bindings ([ADR 0023](0023-key-rendering-and-binding-evaluation.md)), they're stored as presets, evaluated natively, and shown with the expression they compile to:

  | Preset | Compiles to |
  |---|---|
  | **Navigation group** (`nav: "main"`, the default) | `when: self.current.main == me`. Navigation sets `surface.<slug>.current.<group>`. M5's current page is the `main` group |
  | **Levels** (`under`, `normal`, `over`) | `rank` bands of 0, 100 and 200, plus `rank` within the band |
  | **Shift** | `over`, `when: self.controls.<key>.pressed`, or a latching helper |
  | **Popover** | `over`, in its own navigation group, dismissed by navigating that group to nothing |
  | **Tree** (Reactor-style) | `parent`: `when` combined with the parent's, `rank` from position among siblings |
  | **Section** (Buttons-style) | `controls: {region}`, with its own navigation group for folders inside it |
  | **Always on** | `when: true`, usually `under` |

- **Now:**
  - The rename (entity, routes, state root, refs).
  - `controls` as a list, with fit by capability (`doesNotFit`).
  - `surface.<slug>.current.main` in place of `surface.<slug>.page`.
  - Every layer is in the `main` navigation group, which is M5's behaviour.
- **Later:**
  - `on`, `when` and `rank`
  - regions and expressions
  - the resolution rule beyond `main`
  - activation order
  - the other presets

### Control groups (Later)
- **A control group spreads a live list over a set of controls in a layer.** It covers a list of controls or a region. `items` is an expression, `key` gives each item an identity, and its properties are bindings with `item`, `index` and `count` in scope. `overflow` either clips the list or scrolls through it, with the core holding the offset in state.
- **A `control` trigger on `{group}` fires for any of its controls,** with `trigger.item` the item the control was showing. One flow handles the whole grid.
- **Nothing is generated.** A group is one saved entity evaluated over a live list. The core never creates bindings at run time, which would be project edits with revisions, undo and conflicts.
- **Owned controls are exclusive.** A plain binding on a group's control, or two groups that overlap, is `duplicateControl`.

### Behaviour lives in flows; the key editor is a view
- **A control's binding holds its look and its helpers.** What happens when it's used lives in flows that reference it, and a flow can have several triggers ("when k3 is pressed, or when `vh` output 3 changes").
- **Selecting a key shows:**
  - its look, and which layer controls it
  - its helpers
  - every flow that references it, with that flow's other triggers
- **The Button view allows extra triggers,** shown as "Also runs when…", and says when a flow is shared before you edit it.
- **The key editor stays the main way to work:** select a key, then act on it. A flow-first view sits beside it.
- **Now:** the principle. **Later:** the UI.

### Now and Later
- **Now** means [M5 Phase 5](../milestones/m5-surfaces-and-bindings.md), before M5 closes. These changes would otherwise break saved projects, or everything else builds on them.
- **Later** has no milestone yet. Everything Later adds new fields, new values, new entities or new trigger kinds. None of it changes a shape that Now settles.

## Consequences
- **One model for every device.** Supporting a new device is a transport and a capability list. Shift keys, chords and "while held" are conditions over input state.
- **One layer concept covers every known structure.** Companion's pages, Reactor's trees, and Buttons' sections, shift, folders and popovers are presets over one rule. New structures are editor work. Combinations no preset covers can be written directly as expressions.
- **Who controls a key is always explainable.** The answer is a function of current state, so it can be shown live and simulated ("hold k15"). It can't always be predicted while editing once `when` or `rank` are expressions; presets keep the common cases readable.
- **Resolution costs evaluation time.** Every layer's `when` and `rank` is evaluated for every surface it can appear on, and armed layers are rendered ahead of time. Presets are native; expressions use the binding thread.
- **More state and more journal.** Input state, helper values and activation order are all state. Journal sampling (state spec open question 4) has to be decided before helpers ship.
- **Phase 5 changes M5's code:**
  - the attach check, `Layout`, and the fit check in `show`
  - the `page` → `layer` rename across the core, server and UI
  - the input path, which now writes state with a turn per input
  - the gesture recogniser's output, `input.event`
  - `gesture` triggers, fixtures, the virtual deck and the editor

  The binding worker, the renderer and the web surface's framing don't change.
- **Layers are less portable until interfaces or regions arrive.** Free-form IDs mean a layer made for one vendor's controller won't fit another's.

## Alternatives considered
- **Keep the `k`-grid and map positions across sizes:** only keypads ever fit.
- **Input as a separate stream of signals, with interpreters** (an earlier draft of this ADR): two models for device data.
- **Input as state only, with counters for every occurrence:** counters are events in disguise, and they don't work for layer-relative input.
- **A fixed stack of pages, or recency alone:** a fixed stack can't express "the last mode turned on wins", and recency alone loses an always-on strip on the first navigation. Both are presets here.
- **A layer tree with switch nodes at run time (Reactor-style):** scoping is built in, but a tree is one structure among several. Switch nodes add nothing that a navigation group doesn't, and moving a branch changes behaviour. The tree is a preset and an editor view here.
- **Sections on a canvas at run time (Buttons-style):** good for spatial layouts, but a second concept beside layers. Here a section is a layer with a region and its own navigation group.
- **Conditional bindings with CSS-like specificity:** the most flexible, but the hardest to predict, with no structure for the editor to show.
- **Generating bindings at run time for dynamic grids:** that makes the core edit the project. Control groups evaluate a live list instead.
- **Layering per property** (a shift layer inherits the colour underneath): it fits shift layers, but is wrong for navigation, and two rules for one control are hard to explain. A binding can read the look underneath explicitly later if it's needed.
- **Merge surfaces into connections now:** the plugin host doesn't carry surfaces yet. `provider` gets most of the benefit later.
- **Keep value mappings as their own entity:** a continuous control could only drive an action parameter, and one control would need two entities.
- **Helpers in plugins:** inconsistent behaviour, no core timing guarantees, and `why` stopping at the plugin.
