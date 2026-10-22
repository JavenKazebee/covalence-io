# 0022 – Surfaces and pages are core entities; the web surface speaks the surface protocol

- **Status:** Accepted
- **Date:** 2026-10-22
- **Specs:** [surfaces](../specs/surfaces.md), [bindings](../specs/bindings.md), [plugin protocol §8](../specs/plugin-protocol.md)
- **Related:** [ADR 0010](0010-orbital-remote-nodes.md), [ADR 0012](0012-simulated-devices.md), [ADR 0020](0020-project-entity-types-in-the-core.md)
- **Milestone:** [M5: Surfaces and bindings](../milestones/m5-surfaces-and-bindings.md)

## Context
- M5 adds the first surface: a virtual Stream Deck in the browser ([ADR 0012](0012-simulated-devices.md)). Hardware decks and Orbitals come later, and [ADR 0010](0010-orbital-remote-nodes.md) says all of them, browser surfaces included, should reach the core through the same surface protocol (plugin protocol §8).
- The bindings spec defines `control_binding` but not the `surface` and `page` entities it refers to, or how a page ends up on a surface.
- The browser can't use the plugin host's transport: the host is a child process speaking framed MessagePack over stdio. The UI already has one WebSocket (`/ws`) for live state and the journal.
- Gestures (press, double, long, hold) are computed in the core from the host's timestamps (plugin protocol §8). Their exact rules were never written down.

## Decision
- **`surface` and `page` are project entities** in `covalence-core`, beside `control_binding`, with slugs for their state keys as flows have ([surfaces spec §1](../specs/surfaces.md)).
- **A surface entity exists before the device attaches.** The virtual surface is created in the UI and then connects. Hardware will be matched on attach by model and serial.
- **Pages address controls by ID, in row-major order** (`k0`, `k1` …). In M5 a page fits only a surface with the same layout. That keeps one model (15 keys) simple and leaves mapping across sizes open.
- **The current page is state,** `surface.<slug>.page`, set from `startPage` on attach and changed through the API. It's journaled like any other state change, so it's visible to bindings, expressions and `why`. Flows can't change it in M5.
- **The core's surface module is independent of transport.** A transport attaches a surface with its `ControlDef`s, feeds it input and receives draws. The web surface is the first transport. The plugin host and Orbitals will be the next, through the same module.
- **The web surface uses its own WebSocket, `/surface/{id}`,** carrying plugin protocol §8 notifications as JSON, with images as base64 PNG. One socket is one surface, and the socket closing is the detach.
- **Gestures follow fixed rules** ([surfaces spec §3](../specs/surfaces.md)): `press` fires on `down` straight away, and `double` fires as well as `press`, never instead of it. Timing comes from the host's `ts`; `long` and `hold` use a core timer started when the `down` arrives.

## Consequences
- The browser deck exercises the same core path as a hardware deck will: attach, `ControlDef`s, input with device timestamps, rendered images. What the virtual deck shows is what a hardware deck would show.
- A key press reaches the core over one hop with no batching, which keeps the 5 ms key-down budget realistic.
- `/ws` stays a read-only feed for the editor. A UI session that both edits and shows a deck opens two sockets.
- Base64 PNG over JSON costs a third more bytes than binary frames. At 15 keys of about 10 KB on a LAN that doesn't matter. Binary frames can come with the plugin host transport, which already carries bytes.
- A flow that wants to act only on a double-press has to listen for `double` alone. Making `press` wait for the double window would cost 300 ms on every press, which the 5 ms budget rules out.
- Operators can't build a "next page" button yet. Core-provided actions are an open question ([surfaces spec §6](../specs/surfaces.md)).

## Alternatives considered
- **Surfaces over `/ws`:** one socket per UI session. But `/ws` coalesces and batches for the editor, its messages are a different protocol, and a deck in a kiosk browser needs none of the editor's feed.
- **The virtual deck in the plugin host:** the browser would talk to a host plugin, which talks to the core. That's the Orbital path, and it adds a hop and a process for a device that's already talking to the core's own server.
- **Pages addressed by grid position (`{x, y}`):** a page could fit any surface at least its size. Worth doing with a second model; with one model it adds a mapping that nothing tests.
- **The current page as an entity field:** survives a restart, but a page switch would be a project edit (revision, undo, multi-user conflict) rather than live state.
- **`press` held back for the double window, or a separate `tap`:** the first fails the latency budget. The second adds a gesture whose meaning depends on timing nobody sees; it can be added later if users ask.
