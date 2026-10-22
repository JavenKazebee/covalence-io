# Milestone 5: Surfaces and bindings

- **Status:** In progress
- **Started:** 2026-10-22

Covalence gets its first control surface. A virtual Stream Deck in the UI ([ADR 0012](../adr/0012-simulated-devices.md)) attaches to the core over the surface protocol ([plugin protocol §8](../specs/plugin-protocol.md)), the same way a hardware deck or an Orbital will later ([ADR 0010](../adr/0010-orbital-remote-nodes.md)). Pressing a key fires a flow through a `gesture` trigger. Keys show live state through display bindings ([bindings spec](../specs/bindings.md) §1–§4), evaluated in the core with the M4 expression engine and rendered by the core as key images. Hardware surfaces, value mappings and page parameters stay out.

## Done when

- [ ] A virtual Stream Deck (15 keys) appears in the Surfaces tab, attached to the core through `surface.attach`, and shows up as `surface.<slug>.connected` in state. Closing the tab detaches it.
- [ ] A page assigned to that surface shows its bound keys. Switching pages through the API updates `surface.<slug>.page` and the keys, and the new page is fully evaluated before it's drawn.
- [ ] A flow with a `gesture` trigger `{control: {page, control: k0}, gesture: press}` routes `vh` when the key is pressed. `why` on its action walks back through `trigger.matched` and `input.gesture` to the `input.raw` entry. `double`, `long` and `hold` are recognised from the host's `ts`, so a double-press stays one gesture however the events arrive.
- [ ] Each binding kind works on a key: `value` (`vh.outputs[1].source` as the label), `template` (`Out 1: {vh.outputs[1].source}`), `rules` (red when source is 3, else grey), `map` (a number to a list of colours) and `expr`. Each re-renders within one scheduler turn of the state it reads changing, and not when unrelated state changes.
- [ ] A binding that throws or returns `undefined` falls back to its `fallback`, then the control's default, and reports the error in the editor and the log (rate-limited) without stopping other bindings. A binding that reads a `stale` value is dimmed.
- [ ] Only keys whose look changed are sent as `surface.draw`. Bindings on pages no surface is showing aren't evaluated.
- [ ] State change → redrawn key image in under 16 ms, and key down → first action dispatched in under 5 ms ([architecture](../architecture.md#performance-budgets-targets-to-test-against)), measured with a full page of `expr` bindings changing at once.
- [ ] The UI edits surfaces' pages and bindings: pick a key, choose a binding kind per property, with the same live preview as flow expressions. A "When pressed" shortcut creates a Button-view flow with that key's `gesture` trigger.
- [ ] A Rust end-to-end test covers the above in CI against a simulated surface and simulated Videohubs, with no hardware.

## Phases

### Phase 0: Decisions and docs
- [x] **Entity shapes** ([surfaces spec §1](../specs/surfaces.md), [ADR 0022](../adr/0022-surfaces-pages-and-the-web-surface.md)). `surface` and `page` are entities with slugs, beside `control_binding`. A surface entity exists before its device attaches. Pages address buttons as `k0`, `k1` … in row-major order, and in M5 a page fits only a surface with the same layout.
- [x] **The current page is state,** `surface.<slug>.page`: `startPage` on attach, then `POST /surfaces/{id}/page`, journaled. Flows can't switch pages in M5; core-provided actions are an open question (surfaces spec §6).
- [x] **The web surface** connects to its own WebSocket, `GET /surface/{id}`, carrying plugin protocol §8 notifications as JSON, with images as base64 PNG. The core's surface module doesn't know its transport, so the plugin host and Orbitals can attach surfaces the same way later (surfaces spec §4).
- [x] **Rendering** ([ADR 0023](../adr/0023-key-rendering-and-binding-evaluation.md)): `tiny-skia` and `cosmic-text`, Inter embedded, PNG only. Black key, white label, wrapped to three lines and shrunk to fit. The virtual deck declares 144 × 144 keys.
- [x] **Gesture timing** (surfaces spec §3): `press` on `down` straight away; `double` as well as `press`, within 300 ms of the previous `up`; `long` at 500 ms; `hold` from 500 ms, every 150 ms. Timing from the host's `ts`, with a core timer for `long` and `hold`.
- [x] **Binding evaluation cost** (ADR 0023). The bytecode prelude is out, since the workspace denies `unsafe`, and so is a frozen context. Instead `value`, `template`, `rules` and `map` are evaluated natively in Rust, tested against their compiled expressions, and `expr` bindings run on a dedicated binding thread with a 10 ms limit.
- [x] **Stale and missing** (ADR 0023): a stale key is drawn at half brightness with a corner marker; an unbound key is blank.
- [x] [Flow spec](../specs/flows.md) §8 lists `gesture` triggers as **(M5)**. [Bindings spec](../specs/bindings.md) has a status note, the new evaluation rule in §3, and open question 1 answered. New [surfaces spec](../specs/surfaces.md).

### Phase 1: Surfaces in the core (Rust)
- [x] `covalence_core::surface` (`Surfaces`): the `surface` and `page` entity types, attached surfaces and their `ControlDef`s, and `surface.<slug>.connected`, `.model` and `.page` written through the journal as `state.patch`. A transport attaches a surface and gets a `Link`: an input sender, whose drop detaches it, and a draw receiver, which closes when a newer attach replaces it. `show` switches pages and checks the layout. A surface that reattaches shows the page it showed last.
- [x] `input.raw` and `input.gesture` journal records. Gestures carry the page shown when the input arrived, and `cause` → the raw input; a timed `long` or `hold` → its `down`; a release on detach → the `connected: false` patch.
- [x] The gesture recogniser (`surface::gesture`), on the host's `ts` with a core timer for `long` and `hold`. Unit tests for every gesture, a late `up`, a missed `up`, doubles per control and across a detach.
- [x] `gesture` triggers in the flow model, validation (`unknownPage`, `unknownControl`; `turn` is `unsupported`) and runtime, matched from `input.gesture` entries beside `event` triggers. `trigger.matched` has the surface as its origin. Fixtures: `gesture.json` and `problems/gestures.json`; `problems/unsupported.json` now uses `startup` and a `turn` gesture.
- [x] The renderer (`surface::render`): `tiny-skia` and `cosmic-text` with Inter SemiBold embedded (`assets/fonts/`, OFL) and no system fonts. A label shrinks before it breaks a word, wraps to three lines, and only then breaks words and cuts off. Stale keys are dimmed with an amber corner. PNG only. Snapshot tests in `tests/key_images.rs` (`UPDATE_SNAPSHOTS=1` to rewrite them).
- [x] A simulated 15-key deck in `tests/surfaces.rs`, attaching through `Surfaces` as the web surface will: state on attach and detach, every key drawn, gestures from input and the timer, replacement, page switching, and a `gesture` trigger whose `set` step's `why` reaches `input.raw`.
- `surface` and `page` are reserved roots in expressions, so a connection alias can't be either.
- Until bindings arrive (Phase 2), every key is drawn blank, and drawing happens on the caller's thread.
- The server passes no pages to flow validation until Phase 3, so a `gesture` trigger can't be saved through the API yet.
- `ControlDef` and `Attach` are Rust types in the core for now. They move into the protocol's TypeSpec when the plugin host carries surfaces.

### Phase 2: Bindings (Rust)
- [x] `covalence_core::binding`: the `control_binding` entity (`ControlBinding`, `Prop`, `Binding`, `Ref` …), `parse` with `unsupported` for later properties and target refs, and `validate` (bindings spec §2.1). Shared fixtures in `fixtures/bindings/`, with problem fixtures for every error code.
- [x] Native evaluation of `value`, `template`, `rules` and `map` (`Binding::evaluate`), and their compiled expressions (`Binding::to_expr`). `binding::js` reproduces the JavaScript rules they need: `String()`, `Number()`, `toFixed`, `Math.round`, `===` and relational comparison. `tests/bindings.rs` runs every fixture, every format, every condition and every map shape over a set of awkward values both ways and requires the same result, and that a native binding reads at least what its expression reads.
- [x] The binding worker (`binding::worker`): its own thread, its own QuickJS with a 10 ms limit, one turn per batch of state changes, only pages some surface shows, a page evaluated in full when it comes into view. Fallbacks, then the control's default. Errors are kept per binding property (`Surfaces::binding_problems`, for the editor) and logged when they change. A stale read dims the key. Each surface is sent only the keys whose look differs from what it last got. `Surfaces` now draws through it: `set_binding`, `remove_binding`, `set_aliases` and `flush`.
- [x] `tests/binding_worker.rs`: a bound key's first draw, redraws only on what it reads, fallbacks and reported errors that clear, stale dimming, hidden pages not evaluated, a dynamic read following `vars`, edits while shown, two surfaces on one page.
- [x] **Benchmark** (`bench_a_full_page_of_expr_bindings`, ignored, run in release), two `expr` bindings per key, every key changing at once, on an Apple M3:

  | Page | Median | Worst |
  |---|---|---|
  | 15 keys at 72 px | 8.4 ms | 10.8 ms |
  | 15 keys at 144 px (the virtual deck) | 8.9 ms | 9.2 ms |
  | 32 keys at 96 px | 18.4 ms | 20.2 ms |

  M5's 15-key deck is inside the 16 ms budget. A 32-key page of `expr` bindings isn't: 64 evaluations at ~270 µs each are most of it. ADR 0023's binding thread pool is the fix when a 32-key surface arrives; structured bindings cost microseconds and don't need it.
- PNG encoding moved from `tiny-skia`'s encoder to `png` with fast compression and RGB: a blank key went from 176 µs to 16 µs.
- The worker draws on its own thread now, so drawing is asynchronous: `Surfaces::flush` waits for it, for tests and, from Phase 3, for the API.

### Phase 3: HTTP API and the surface transport
- [x] Endpoints for surfaces, pages and control bindings ([surfaces spec §5](../specs/surfaces.md)), through the same save path as flows: `POST` creates, `PUT` replaces, `{entity, warnings}` or `422 {errors, warnings}`, slugs from names. `surface::validate_surface` and `validate_page` in the core; a page can't shrink past a bound control. Deleting a page deletes its bindings and clears it as a start page. `POST /surfaces/{id}/page` answers once the keys are sent. Flows now validate `gesture` triggers against saved pages. `surface.*` state is in `GET /state` and `/ws`.
- [x] The web surface, `GET /surface/{id}` (`web_surface`): attach first, then input in and draws out as JSON with base64 PNG. Errors close the socket with 1008 and a reason; a newer attach or deleting the surface closes it with 1000 (surfaces spec §4).
- [x] A binding-specific preview, `POST /control-bindings/preview` (bindings spec §2.2): each property's value, error, compiled expression and reads, the resolved look, and the key image, evaluated by the worker's own code (`binding::worker::preview`) and drawn by the same renderer, so it matches what a surface is sent byte for byte. `GET /control-bindings/problems` lists failing binding properties for the editor.
- [x] `crates/covalence-server/tests/surfaces_api.rs`: validation and storage, a restart, previews, the attach rules, and end to end with a WebSocket deck and two simulated Videohubs: every key drawn on attach as previewed, a press on `k1` routes `vh`, `k0` redraws to the new source and `why` reaches `input.raw`, a change on `vh2` redraws only `k2`, an edited binding redraws, a page switch blanks the bound keys, and closing the socket detaches.
- The spec's `PUT` that creates was dropped for the flows' `POST` and `PUT`, so the UI has one way to save every entity.
- The preview uses its own QuickJS with the binding worker's 10 ms limit, so an `expr` that's too slow for a key fails in the preview too.

### Phase 4: UI
- [ ] The Surfaces tab, with the virtual Stream Deck: shows the core's images and sends key down and up with timestamps.
- [ ] A page editor: the key grid, select a key, edit its binding per property with a kind picker, using `ExprInput` and its preview for `expr` and showing the compiled expression for the others (one-way conversion to `expr`).
- [ ] "When pressed" on a key creates or opens a Button-view flow with its `gesture` trigger.
- [ ] Flow editor support for `gesture` triggers, and journal summaries for `input.raw` and `input.gesture`.
- [ ] TS types and the editor round-trip every page and binding fixture.

### Phase 5: Acceptance
- [ ] The Rust end-to-end tests run in the existing `cargo test` CI job.
- [ ] Checked by hand in the browser: bind a key to a Videohub output, press another key to route it, and watch the first key change.
- [ ] Decide whether the browser smoke test dropped from M3 and M4 comes back, now that pressing a key in the UI is the main path through the product.

## Ordering

Phase 0 comes first. Phases 1 and 2 can run side by side once the entity shapes and rendering stack are decided; Phase 2 needs Phase 1's renderer only for its last step. Phase 3 follows them. Phase 4 can start against the written-down shapes and a stubbed transport. Phase 5 joins them.

## Out of scope

Hardware surfaces and the `covalence-surfaces` driver crate · Orbitals · value mappings, faders and encoders (bindings §5) · page parameters and templates (bindings §6) · plugin presets (bindings §7) · dynamic targets and interfaces ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) · images and the `asset` entity · LEDs, brightness and text-only displays · type checking of `expr` bindings and generated TypeScript definitions · the `variable` entity and persisted variables · multiple users editing a page · everything left out of M4's flows.
