# Milestone 5: Surfaces and bindings

- **Status:** In progress
- **Started:** 2026-10-22

Covalence gets its first control surface. A virtual Stream Deck in the UI ([ADR 0012](../adr/0012-simulated-devices.md)) attaches to the core over the surface protocol ([plugin protocol §8](../specs/plugin-protocol.md)), the same way a hardware deck or an Orbital will later ([ADR 0010](../adr/0010-orbital-remote-nodes.md)). Pressing a key fires a flow through a `gesture` trigger (a `control` trigger after Phase 5). Keys show live state through display bindings ([bindings spec](../specs/bindings.md) §1–§4), evaluated in the core with the M4 expression engine and rendered by the core as key images. Before M5 closes, Phase 5 reshapes controls and triggers so that they aren't tied to a Stream Deck, makes control input device state with events derived from it, and renames pages to layers ([ADR 0024](../adr/0024-surfaces-as-two-way-devices.md)). Hardware surfaces, helpers, layer resolution beyond navigation, and layer parameters stay out.

## Done when

- [ ] A virtual Stream Deck (15 keys) appears in the Surfaces tab, attached to the core through `surface.attach`, and shows up as `surface.<slug>.connected` in state. Closing the tab detaches it.
- [ ] A layer navigated to on that surface shows its bound keys. Navigating through the API updates `surface.<slug>.current.main` (`surface.<slug>.page` before Phase 5) and the keys, and the new layer is fully evaluated before it's drawn.
- [ ] A flow with a `control` trigger `{control: {layer, control: k0}, event: press}` routes `vh` when the key is pressed. `why` on its action walks back through `trigger.matched` and `input.event` to the input's `state.patch` on `surface.<slug>.controls.k0.pressed`. `double`, `long` and `hold` are recognised from the host's `ts`, so a double-press stays one gesture however the events arrive.
- [ ] Each binding kind works on a key: `value` (`vh.outputs[1].source` as the label), `template` (`Out 1: {vh.outputs[1].source}`), `rules` (red when source is 3, else grey), `map` (a number to a list of colours) and `expr`. Each re-renders within one scheduler turn of the state it reads changing, and not when unrelated state changes.
- [ ] A binding that throws or returns `undefined` falls back to its `fallback`, then the control's default, and reports the error in the editor and the log (rate-limited) without stopping other bindings. A binding that reads a `stale` value is dimmed.
- [ ] Only keys whose look changed are sent as `surface.draw`. Bindings on layers no surface is showing aren't evaluated.
- [ ] State change → redrawn key image in under 16 ms, and key down → first action dispatched in under 5 ms ([architecture](../architecture.md#performance-budgets-targets-to-test-against)), measured with a full layer of `expr` bindings changing at once.
- [ ] The UI edits surfaces' layers and bindings: pick a key, choose a binding kind per property, with the same live preview as flow expressions. A "When pressed" shortcut creates a Button-view flow with that key's `control` trigger.
- [ ] Controls are typed by capability and placed with `at`, and layers list their controls ([ADR 0024](../adr/0024-surfaces-as-two-way-devices.md)): the virtual deck attaches 15 `momentary` + `image` controls with no layout, a layer made from it fits it, and a layer that needs a control the surface lacks is refused with `doesNotFit`.
- [ ] A Rust end-to-end test covers the above in CI against a simulated surface and simulated Videohubs, with no hardware.

## Phases

### Phase 0: Decisions and docs
- [x] **Entity shapes** ([surfaces spec §1](../specs/surfaces.md), [ADR 0022](../adr/0022-surfaces-pages-and-the-web-surface.md)). `surface` and `page` are entities with slugs, beside `control_binding`. A surface entity exists before its device attaches. Pages address buttons as `k0`, `k1` … in row-major order, and in M5 a page fits only a surface with the same layout. Reshaped by Phase 5.
- [x] **The current page is state,** `surface.<slug>.page`: `startPage` on attach, then `POST /surfaces/{id}/page`, journaled. Flows can't switch pages in M5; core-provided actions are an open question (surfaces spec §10).
- [x] **The web surface** connects to its own WebSocket, `GET /surface/{id}`, carrying plugin protocol §8 notifications as JSON, with images as base64 PNG. The core's surface module doesn't know its transport, so the plugin host and Orbitals can attach surfaces the same way later (surfaces spec §8).
- [x] **Rendering** ([ADR 0023](../adr/0023-key-rendering-and-binding-evaluation.md)): `tiny-skia` and `cosmic-text`, Inter embedded, PNG only. Black key, white label, wrapped to three lines and shrunk to fit. The virtual deck declares 144 × 144 keys.
- [x] **Gesture timing** (surfaces spec §4.2): `press` on `down` straight away; `double` as well as `press`, within 300 ms of the previous `up`; `long` at 500 ms; `hold` from 500 ms, every 150 ms. Timing from the host's `ts`, with a core timer for `long` and `hold`.
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
- [x] Endpoints for surfaces, pages and control bindings ([surfaces spec §9](../specs/surfaces.md)), through the same save path as flows: `POST` creates, `PUT` replaces, `{entity, warnings}` or `422 {errors, warnings}`, slugs from names. `surface::validate_surface` and `validate_page` in the core; a page can't shrink past a bound control. Deleting a page deletes its bindings and clears it as a start page. `POST /surfaces/{id}/page` answers once the keys are sent. Flows now validate `gesture` triggers against saved pages. `surface.*` state is in `GET /state` and `/ws`.
- [x] The web surface, `GET /surface/{id}` (`web_surface`): attach first, then input in and draws out as JSON with base64 PNG. Errors close the socket with 1008 and a reason; a newer attach or deleting the surface closes it with 1000 (surfaces spec §8).
- [x] A binding-specific preview, `POST /control-bindings/preview` (bindings spec §2.2): each property's value, error, compiled expression and reads, the resolved look, and the key image, evaluated by the worker's own code (`binding::worker::preview`) and drawn by the same renderer, so it matches what a surface is sent byte for byte. `GET /control-bindings/problems` lists failing binding properties for the editor.
- [x] `crates/covalence-server/tests/surfaces_api.rs`: validation and storage, a restart, previews, the attach rules, and end to end with a WebSocket deck and two simulated Videohubs: every key drawn on attach as previewed, a press on `k1` routes `vh`, `k0` redraws to the new source and `why` reaches `input.raw`, a change on `vh2` redraws only `k2`, an edited binding redraws, a page switch blanks the bound keys, and closing the socket detaches.
- The spec's `PUT` that creates was dropped for the flows' `POST` and `PUT`, so the UI has one way to save every entity.
- The preview uses its own QuickJS with the binding worker's 10 ms limit, so an `expr` that's too slow for a key fails in the preview too.

### Phase 4: UI
- [x] The Surfaces tab (`#/surfaces`): surfaces and pages, a "New virtual deck" and a "New page". A surface's screen (`#/surfaces/<id>`) is its virtual Stream Deck (`VirtualDeck`, `surfaces/deck.ts`), attached over `/surface/{id}` while the screen is open: it shows the core's images and sends key down and up with `performance.now()`, from the mouse or Space and Enter. It reattaches after a dropped connection, but not after `attached elsewhere` or a 1008, so two tabs don't fight over one surface. The page it shows is picked there, through `POST /surfaces/{id}/page`.
- [x] A page editor (`#/surfaces/pages/<id>/<control>`): the key grid drawn by the core through the preview endpoint, then per property a kind picker and its form. Refs are typed as text paths (`vh.outputs[1].source`) and stored by connection ID; a template is text with `{path|format}` placeholders. `expr` uses `ExprInput`; the other kinds show the expression they compile to, and "Edit as expression" converts, with "Back" only while the source is untouched. The key's preview, and every key in the grid, are previewed again when state they read changes (`bindings/usePreviews.ts` subscribes to their `reads`). Evaluation errors from `/control-bindings/problems` show under their property.
- [x] "When pressed" on a key opens the flow with that key's `press` trigger, or a new Button-view flow with it filled in.
- [x] Flow editor support for `gesture` triggers ("Add key trigger": page, key and gesture), and journal summaries for `input.raw` and `input.gesture`, with surfaces and pages by name.
- [x] TS types for surfaces, pages and bindings (`surfaces/model.ts`, `bindings/model.ts`). `bindings/model.test.ts` round-trips every binding fixture through the editor, and every ref, template and condition value through its text form; `flows/model.test.ts` already round-trips `gesture.json`.
- There are no page fixtures to round-trip: pages are three fields, covered by the Rust API tests.
- The editor's previews are of saved bindings, plus the selected key's draft. A surface showing the page gets the same images, since the preview is drawn by the binding worker's own code.

### Phase 5: Device model ([ADR 0024](../adr/0024-surfaces-as-two-way-devices.md))
The parts of ADR 0024 marked **Now**. They're the shape changes that would otherwise break saved projects, plus the input state and events everything Later builds on. Capabilities beyond M5's keys are accepted and stored; their outputs do nothing yet.
- [x] **Capability-typed controls** ([surfaces spec §2](../specs/surfaces.md), [plugin protocol §8](../specs/plugin-protocol.md)):
  - `ControlDef` gets `label?`, a required `at`, `inputs` and `outputs`, replacing `kind`, `x`/`y`/`w`/`h`, `image` and `pressable`.
  - IDs are free-form, and the attach has no `layout`.
  - `check_attach` refuses duplicate IDs, a missing `at`, overlapping controls, and controls with no capabilities.
  - Input that doesn't match a control's capabilities is dropped and logged.
  - Only the `image` output does anything.
  - Done: `ControlDef` with `Placement`, `InputCap` and `OutputCap`. `check_attach` also refuses a control with no size, two inputs or outputs of one kind, an image outside 1 to 1024 px, and more than 1024 controls. Until pages become layers, a page fits a surface that has each of its `k` controls with `momentary` and `image`, and `show` answers `DoesNotFit` with the missing ones (`409 doesNotFit` with `controls`).
- [x] **Input state** (surfaces spec §3, §4.1):
  - `surface.input` becomes a `state.patch` on `surface.<slug>.controls.<id>.{pressed, velocity, detents, value, touched}`, with the surface as origin and `deviceTs`, replacing `input.raw`.
  - Key, touch and encoder input each get a scheduler turn of their own; `value` input may coalesce.
  - The state is volatile: reset on attach, released on detach with `cause` → the `connected: false` patch, not restored after a restart.
  - Done: `state.patch` has `deviceTs`. A scheduler turn of its own is `LiveState::subscribe_turns` and `seen`: the flow runtime's `state` triggers and `waitUntil` say when they've evaluated a change, and the surface waits for that (up to 50 ms) before the next edge. A `down` while pressed is a release, then a press. Bindings aren't waited for. `velocity` is written with the `down`; `turn` must be whole detents; `value` is clamped to 0 to 1.
- [x] **Events** (surfaces spec §4.2): the gesture recogniser reads input state and journals `input.event` (`press`, `release`, `double`, `long`, `hold`, `turn`, `change`, `touch`, `untouch`) with `cause` → the patch and the controlling `layer`, replacing `input.gesture`.
  - Done: `gesture::Change` in, `ControlEvent` out. A `release` carries its press's layer, even after the page changes. Until pages become layers, `layer` is the shown page's ID and `gesture` triggers match on it; `turn`, `change`, `touch` and `untouch` gestures are `unsupported`. The UI's deck attaches with capabilities and `at`, and the journal summarises `input.event`.

- [ ] **Pages become layers** (surfaces spec §1, §5):
  - The `page` entity becomes `layer`, with `controls` replacing `layout`, and `Layout` goes. Routes move from `/pages` to `/layers`.
  - The `page` state root becomes `layer`, and `startPage` becomes `startLayer`.
  - `surface.<slug>.page` becomes `surface.<slug>.current.main`, and `POST /surfaces/{id}/page` becomes `POST /surfaces/{id}/current`.
  - Navigation checks the fit by capability and answers `409 doesNotFit` with the missing controls, instead of `layoutMismatch`.
  - `GET /surfaces/{id}/controls` returns what the surface last attached with.
  - Layer validation: unique control IDs, at most 256, bound controls still present. `unknownPage` becomes `unknownLayer`.
  - Every layer is in the `main` navigation group, which is M5's behaviour.
- [ ] **`control` triggers** (surfaces spec §7, [flow spec §8.2](../specs/flows.md)):
  - `gesture` triggers become `{kind: "control", control: {layer, control}, event}`.
  - An event the control's inputs can't give is `unsupported`.
  - A `state` trigger over `pressed` sees every press, however short.
  - Fixtures renamed and updated.
- [ ] **Bindings** ([bindings spec §1](../specs/bindings.md)): `page` becomes `layer`. A property can only be bound on a layer control that declares its output (`noOutput`). M5's three properties need `image`.
- [ ] **UI:**
  - "Pages" become "Layers" throughout the Surfaces tab.
  - The virtual deck attaches with capabilities and `at`.
  - "New layer" on a surface copies its controls, and the layer editor draws the grid from `at`.
  - "Add key trigger" uses `control` and `event`.
  - Journal summaries cover input patches and `input.event`, and the state view shows control input.
- [ ] **Tests:**
  - the gesture recogniser's unit tests against input state
  - a tap shorter than a scheduler turn still fires a `state` trigger on `pressed` twice (rising, then falling)
  - input state is released on detach and absent after a restart
  - a `state` trigger on two keys' `pressed` works as a chord
  - an attach without `at`, or with overlapping controls, is refused
- No migration: no saved projects use M5's shapes yet. A development database is reset.

### Phase 6: Acceptance
- [ ] The Rust end-to-end tests run in the existing `cargo test` CI job.
- [ ] Checked by hand in the browser: bind a key to a Videohub output, press another key to route it, and watch the first key change.

## Ordering

Phase 0 comes first. Phases 1 and 2 can run side by side once the entity shapes and rendering stack are decided; Phase 2 needs Phase 1's renderer only for its last step. Phase 3 follows them. Phase 4 can start against the written-down shapes and a stubbed transport. Phase 5 reshapes what Phases 1–4 built, and Phase 6 joins them.

## Out of scope

Hardware surfaces and the `covalence-surfaces` driver crate · Orbitals · helpers, routes from faders and encoders to actions (bindings §5) · pattern `state` triggers · reported and requested values, and echoes · injecting input · layer `on`, `when`, `level` and `rank`, the resolution rule and its presets (shift, popover, tree, section, navigation groups beyond `main`) · control groups · region and relative refs · surface groups · the key editor's list of flows and Button view chips · surface, selector and expression control refs · surfaces provided by connections, and surface interfaces · layer parameters and templates (bindings §6) · plugin presets (bindings §7) · dynamic targets and interfaces ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)) · images and the `asset` entity · LEDs, brightness and text-only displays · type checking of `expr` bindings and generated TypeScript definitions · the `variable` entity and persisted variables · multiple users editing a page · everything left out of M4's flows.
