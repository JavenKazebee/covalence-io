# 0001 – Core engine in Rust

- **Status:** Accepted. Device drivers moved to plugins by [0025](0025-device-drivers-are-plugins.md); the core performs their hardware I/O.
- **Date:** 2026-10-18

## Context
The engine runs live shows for 12+ hours at a time on Windows, macOS, Linux and Raspberry Pi. It must be stable, keep latency low and predictable, and run headless. It talks to USB HID, MIDI and serial hardware, renders surface images, and supervises plugin processes.

## Decision
Write `covalence-core` in Rust on tokio. The core owns:
- the state tree, the runtime journal and the project store
- the flow runtime, scheduler, gesture layer and output gate
- the script engine (QuickJS; see [0004](0004-javascript-expressions-and-scripts.md))
- surface rendering (`tiny-skia` / `resvg`) and built-in drivers: Stream Deck HID, MIDI, OSC, TCP/UDP/HTTP/WebSocket, serial
- the HTTP/WebSocket API (`axum`), which serves the UI
- supervision of plugin hosts and Orbitals

## Consequences
- No garbage-collection pauses, low memory use, and a small single binary that is easy to cross-compile.
- Show-critical paths (input → gesture → trigger → output) never depend on the plugin runtime.
- Two languages in the project (Rust and TypeScript), and core work moves more slowly than it would in TypeScript. Plugin authors never need Rust.

## Alternatives considered
- **All TypeScript (Node/Bun):** fastest to build, but GC and memory growth over long shows, and weaker isolation.
- **Go:** simpler concurrency, but weaker HID and image-rendering libraries, and little advantage over Rust.
- **.NET:** good performance, but a smaller ecosystem in this field and a weaker story for headless use and a remote UI.
