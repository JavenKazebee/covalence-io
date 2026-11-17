# 0035 – Level outputs, echoes, gesture timing and per-surface helpers

- **Status:** Proposed
- **Date:** 2026-11-17
- **Answers:** the rest of [M8 Phase 4](../milestones/m8-dataflow.md): echoes, the virtual encoder and fader, `gestures`, and helpers with `scope: "surface"`
- **Specs:** [surfaces §2, §4.2–§4.4, §8](../specs/surfaces.md), [bindings §1, §5](../specs/bindings.md), [plugin protocol §8](../specs/plugin-protocol.md), [dataflow §3.1, §5](../specs/dataflow.md)
- **Milestone:** [M8: Dataflow](../milestones/m8-dataflow.md), Phase 4

## Context
- **Only `image` outputs do anything.** A motor fader, an encoder's light ring and a meter need a number sent to them: the `level` output (surfaces §2), which the bindings' `value` property drives (bindings §1). The plugin protocol sketches `surface.level` as Later.
- **A motor fader reports its own movement.** Told to go to 70%, it reports 40%, 55%, 70% on the way, exactly as if someone were pushing it. Taken as input, those reports go to the mixer, come back as a volume change, move the fader again, and the fader fights itself. Surfaces §4.4 calls them **echoes** and says they must update the fader's state without acting as input, but leaves how to recognise them, and what's kept of them, open.
- **Without hardware there's nothing to test this on.** M8's acceptance needs an encoder and a motor fader, and the virtual surface is a fixed 15-key deck.
- **Gesture timing is global** (300 ms double, 500 ms long, 150 ms hold). Bindings §1 gives each control binding a `gestures` field, as sources, without saying when they're read.
- **Helpers are shared by every surface a layer is on.** `scope: "surface"` (surfaces §4.3) gives each surface its own, but nothing says when such a cell exists.

## Decision

### `value` and `level`
- **Control bindings gain the `value` property:** a number from 0 to 1, driving a `level` output. Like `label` and `image`, it needs the output on the layer's control (`noOutput`), and it's evaluated by the binding worker beside the look.
- **Outputs, not draws.** The surface link's draw channel carries an `Output`: an image, or a level. One channel per surface keeps one order and one rule for a slow device: a value replaced before it was sent isn't sent.
- **`surface.level {surface, control, value}`** (core → host) is implemented as plugin protocol §8 sketches it: in the TypeSpec and its generated types, the SDK (`onLevel`), the plugin host and the web surface. `led` and `text` stay Later and will take the same path.
- **What was sent is state.** Each level sent is written to `surface.<slug>.controls.<id>.requested.level`, with `cause` → the latest change among what the binding read. `why` on a fader's movement reaches the volume change that moved it.

### Echoes
- **An input is an echo** when either:
  - the host marks it `echo: true`
  - it's a `value` that arrives within `echoWindow` (default 100 ms) of a level sent to the same control, and is within `echoTolerance` (default 0.01) of it. Motor faders and 7-bit MIDI never land exactly where they were sent.
- **Input while the control is touched is never an echo.** A hand on the fader means a person is moving it, whatever was just sent.
- **The window is on the core's clock,** from sending to receiving. A device's timestamps can't be compared with the core's.
- **An echo updates input state** (`value`) with `cause` → the `requested.level` patch.
- **Echoes are journaled, marked.** Their `change` event is journaled with `echo: true`. Event triggers and reducers skip echo events unless they set `echo: true` (dataflow §3.1's `EventSpec.echo`, brought forward from Later). So an `absolute` helper on the fader doesn't step on its motor's movement, and its sink sends nothing back, while the timeline still shows what the fader did.

### The configurable virtual surface
- **The virtual surface entity gains `virtual`:** `{encoders?: 0–4, faders?: 0–4, motor?: boolean}`, default none, none and `true`. Without it, a virtual surface is today's 15-key deck, so existing surfaces don't change.
- **Layout:** the keys stay 5 × 3 (`k0`…`k14`).
  - Encoders `e0`…`e3` form a row beneath them, each `relative` + `momentary` with a non-motor `level` output for its ring.
  - Faders `f0`…`f3` stand to the right of the keys, each three cells tall, `absolute` + `touch` with a `level` output, `motor` as configured.
- **A motorised virtual fader moves itself:** on `surface.level` it glides to the value and reports `value` input on the way, unmarked, as hardware does. That exercises the echo window.
- **Changing `virtual` re-attaches** the browser with the new controls. A layer that uses a control the surface no longer has gets the existing `doesNotFit`.
- **Core tests** attach the same controls in process, without a browser.

### `gestures`
- **`gestures` on a control binding:** `{doubleWindow?, longAfter?, holdEvery?, echoWindow?, echoTolerance?}`. Each is a `Source`: a duration (`400ms`) or a number of milliseconds, a `{ref}` or an `{expr}`. Tolerance is a number.
- **They're read when they're needed, not followed** (one-shot positions, dataflow §1.2):
  - `longAfter` and `holdEvery` when the control goes down
  - `doubleWindow` at the next press
  - `echoWindow` and `echoTolerance` when a value arrives

  So they cost nothing between presses, and a change ("novice mode" on) applies from the next press, never during one.
- **The timing for a control comes from the binding on the layer controlling it on that surface** at the time it's read. A press keeps the timing it started with, even if the layer changes during it.
- **A failure or a value that isn't a duration** gives that setting its default, and is reported like a failing binding property.

### Helpers with `scope: "surface"`
- **One cell per surface,** at `surface.<slug>.controls.<id>.helpers.<name>`, identified as `helper:<binding>/<key>@<surface id>`.
- **Its `on` defaults to `{surface, controls: <id>}`,** filtered to input that arrived while the helper's layer controlled the control on that surface.
- **It exists from the first time a surface shows the layer** until the binding or the surface is deleted. A surface's value survives navigating away and back.

## Consequences
- The protocol gains its first output besides images. Plugins that drive faders or rings get `onLevel` in the SDK; others ignore it.
- Echo handling lives in the surface module, beside gesture recognition, so it applies to every surface, plugin or web.
- The journal shows a motor fader's movement as marked echoes. A flow that wants it opts in with `echo: true`.
- Gesture timing becomes per control, at the cost of an evaluation per press when a control's timing isn't a literal.
- Surface-scoped helpers make cells appear as surfaces show layers, and stay. A deleted surface takes its helpers' values with it.

## Alternatives considered
- **A second fixed `virtual.console` model** for the encoder and fader: simpler to lay out, but every new mix of controls would need another model.
- **Not journaling echoes:** leaner, but the timeline wouldn't show the fader's own moves, and nothing could react to them.
- **Echoes only when the host marks them:** exact for devices that do, but most MIDI and OSC hardware can't.
- **Gesture timing as literals only:** simpler, but a mode that slows double-presses for a new operator would need every binding edited.
- **Surface-scoped helpers that reset when their layer leaves the surface:** fewer cells, but a deck's toggles would forget themselves on every page change.
