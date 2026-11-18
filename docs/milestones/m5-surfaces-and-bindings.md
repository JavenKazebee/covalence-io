# Milestone 5: Surfaces and bindings

- **Status:** Done
- **Started:** 2026-10-22

Covalence gets its first control surface. A virtual Stream Deck in the UI ([overview: simulated devices](../specs/overview.md#simulated-devices)) attaches to the core over the surface protocol ([plugin protocol §8](../specs/plugin-protocol.md)), the same way a hardware deck or an Orbital will later ([overview: Orbital](../specs/overview.md#orbital-remote-nodes-not-built)). Pressing a key fires a flow through a `gesture` trigger (a `control` trigger after Phase 5). Keys show live state through display bindings ([bindings spec](../specs/bindings.md) §1–§4), evaluated in the core with the M4 expression engine and rendered by the core as key images. Before M5 closes, Phase 5 reshapes controls and triggers so that they aren't tied to a Stream Deck, makes control input device state with events derived from it, and renames pages to layers ([surfaces §11.2](../specs/surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state)). Hardware surfaces, helpers, layer resolution beyond navigation, and layer parameters stay out.

## Done when

- [x] A virtual Stream Deck (15 keys) appears in the Surfaces tab, attached to the core through `surface.attach`, and shows up as `surface.<slug>.connected` in state. Closing the tab detaches it.
- [x] A layer navigated to on that surface shows its bound keys. Navigating through the API updates `surface.<slug>.current.main` (`surface.<slug>.page` before Phase 5) and the keys, and the new layer is fully evaluated before it's drawn.
- [x] A flow with a `control` trigger `{control: {layer, control: k0}, event: press}` routes `vh` when the key is pressed. `why` on its action walks back through `trigger.matched` and `input.event` to the input's `state.patch` on `surface.<slug>.controls.k0.pressed`. `double`, `long` and `hold` are recognised from the host's `ts`, so a double-press stays one gesture however the events arrive.
- [x] Each binding kind works on a key: `value` (`vh.outputs[1].source` as the label), `template` (`Out 1: {vh.outputs[1].source}`), `rules` (red when source is 3, else grey), `map` (a number to a list of colours) and `expr`. Each re-renders within one scheduler turn of the state it reads changing, and not when unrelated state changes.
- [x] A binding that throws or returns `undefined` falls back to its `fallback`, then the control's default, and reports the error in the editor and the log (rate-limited) without stopping other bindings. A binding that reads a `stale` value is dimmed.
- [x] Only keys whose look changed are sent as `surface.draw`. Bindings on layers no surface is showing aren't evaluated.
- [x] State change → redrawn key image in under 16 ms, and key down → first action dispatched in under 5 ms ([overview](../specs/overview.md#performance-budgets-targets-to-test-against)), measured with a full layer of `expr` bindings changing at once.
- [x] The UI edits surfaces' layers and bindings: pick a key, choose a binding kind per property, with the same live preview as flow expressions. A "When pressed" shortcut creates a Button-view flow with that key's `control` trigger.
- [x] Controls are typed by capability and placed with `at`, and layers list their controls ([surfaces §11.2](../specs/surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state)): the virtual deck attaches 15 `momentary` + `image` controls with no layout, a layer made from it fits it, and a layer that needs a control the surface lacks is refused with `doesNotFit`.
- [x] A Rust end-to-end test covers the above in CI against a simulated surface and simulated Videohubs, with no hardware.

## Budgets measured

On an Apple M3, in release, at the end of M5:

| Path | Budget | Median | Worst |
|---|---|---|---|
| State change → 15 keys at 144 px redrawn, 2 `expr` bindings each | 16 ms | 8.7 ms | 9.2 ms |
| The same at 72 px | 16 ms | 9.0 ms | 13.2 ms |
| Key down → first action requested, with that layer redrawing | 5 ms | 24.5 µs | 82 µs |

`bench_key_down_to_first_action` is new, beside `bench_a_full_page_of_expr_bindings`. It times from `surface.input` to the flow runtime's action request, so it leaves out the trip to the plugin host and back, which nothing measures yet. A 32-key layer of `expr` bindings still takes 18 ms, over budget, as in Phase 2.

## History

The phase-by-phase build log was trimmed once the milestone closed. It's in git: `git log -p -- docs/milestones/m5-surfaces-and-bindings.md`.
