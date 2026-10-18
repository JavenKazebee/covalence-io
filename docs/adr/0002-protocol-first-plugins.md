# 0002 – Plugins defined protocol-first

- **Status:** Accepted. The choice of schema tool is still Proposed.
- **Date:** 2026-10-18

## Context
The device ecosystem decides whether this project succeeds. Plugins must be able to:
- run in different hosts (the shared Deno host, isolated processes, Orbital nodes, and later WASM components or other languages)
- outlive any one runtime choice

## Decision
1. Define a **versioned, language-neutral plugin protocol** ([spec](../specs/plugin-protocol.md)). It is the contract; every SDK and host is an implementation of it.
2. **Source of truth:** the schema lives in `protocol/` and generates both Rust and TypeScript types. The proposed tool chain is TypeSpec → JSON Schema → `typify` (Rust) and `json-schema-to-typescript` (TS). Decide this when the first schema is written.
3. **Wire format:** length-prefixed MessagePack frames, with a newline-delimited JSON debug mode.
4. Plugins declare a **manifest** (config, permissions, actions, events, state schema, presets). The UI generates forms and autocomplete from it.
5. The SDK offers **host transports** (`ctx.tcp`, `ctx.udp`, `ctx.ws`, `ctx.http`, `ctx.serial`). The core performs the I/O, so it can:
   - enforce permissions
   - record traffic for the inspector and for record/replay
   - reconnect automatically

   Transports are optional; plugins can still use raw runtime APIs.
6. A TypeScript SDK (`@covalence-io/sdk`) is the first-class SDK. Other languages implement the same protocol.

## Consequences
- The runtime can change (Deno → Node → WASM) without rewriting plugins.
- Orbital is just another host, with nothing special-cased in the core.
- Plugins that use only host transports can later run in WASM unchanged.
- The protocol is a public API, so breaking changes need a major version and a deprecation window.

## Alternatives considered
- **Runtime-specific plugin API** (Companion's model): faster to start, but ties the ecosystem to one runtime.
- **gRPC/protobuf:** strong tooling, but heavier, and awkward for dynamic JSON-shaped state and schemas.
- **Bitfocus Companion module compatibility:** rejected as a foundation. Modules are built around buttons, have no first-class events and only loosely typed variables. A compatibility adapter may be added later as a separate, lower-tier layer.
