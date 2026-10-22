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
- [ ] A `covalence_core::surface` module: attached surfaces and their `ControlDef`s, `surface.<slug>.connected` and `.page` state, and `input.raw` journaling.
- [ ] The gesture recogniser: `input.raw` → `input.gesture` with `cause`, using the host's `ts`. Unit tests for every gesture, including out-of-order and late events.
- [ ] `gesture` triggers in the flow model and runtime, with validation that the control exists on the page.
- [ ] The renderer: a control's resolved look (label, text colour, background) → an image at its native size and format. Snapshot tests.
- [ ] A simulated surface in the test harness that speaks `surface.*`.

### Phase 2: Bindings (Rust)
- [ ] `page` and `control_binding` entity types and fixtures ([ADR 0020](../adr/0020-project-entity-types-in-the-core.md)), including problem fixtures for each validation error.
- [ ] The five binding kinds, with `value`, `template`, `rules` and `map` compiled to expressions as the spec says. Literal values checked against each property's type; `expr` results checked at run time.
- [ ] The binding scheduler: read sets per binding, re-evaluated once per scheduler turn when touched, only for visible pages; fallbacks and rate-limited errors; stale dimming.
- [ ] Diffing resolved looks, so only changed controls are drawn.
- [ ] Native evaluation of `value`, `template`, `rules` and `map`, tested against their compiled expressions; `expr` bindings on the binding thread; the benchmark against the 16 ms budget.

### Phase 3: HTTP API and the surface transport
- [ ] Endpoints for surfaces, pages and control bindings, through the same save and validation path as flows.
- [ ] The web surface transport chosen in Phase 0.
- [ ] `POST /expressions/evaluate` previews bindings too, or a binding-specific preview that returns the resolved look and rendered image.
- [ ] End-to-end tests in `crates/covalence-server/tests/` with a simulated surface and two simulated Videohubs: a press routes, a binding redraws, `why` reaches `input.raw`.

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
