# Milestone 1: Walking skeleton

- **Status:** In progress
- **Started:** 2026-10-18

Route a Blackmagic Videohub in simulate mode, end to end: API → core → output gate → Deno plugin host → plugin → state patch → journal. Every layer of the architecture is touched; no layer is built out.

## Done when

- [ ] `POST /actions` with `vh.route {output: 1, input: 3}` journals `action.requested` → `action.dispatched` → `action.result`.
- [ ] The plugin's `state.patch` updates `vh.outputs[1].source`.
- [ ] `GET /journal/{seq}/why` walks the cause chain back to the API call.
- [ ] Killing the Deno host process makes the core restart it and resync state in under 3 s.
- [ ] All of the above runs in CI with no hardware.

## Phases

### Phase 0: Decisions and docs
- [x] Defer plugin permission enforcement ([ADR 0015](../adr/0015-defer-plugin-permission-enforcement.md)).
- [x] Schema tool: TypeSpec → JSON Schema, with a hand-written serde fallback ([ADR 0016](../adr/0016-protocol-schema-tooling.md)).
- [x] State paths on the wire are segment arrays ([ADR 0017](../adr/0017-wire-paths-as-segment-arrays.md)).

### Phase 1: Repo scaffold
- [x] Cargo workspace: `covalence-protocol`, `covalence-core`, `covalence-store`, `covalence-server`.
- [x] Deno workspace: `packages/protocol`, `packages/sdk`, `packages/plugin-host`, `plugins/videohub`.
- [x] pnpm workspace: `protocol/` (TypeSpec tooling). The UI joins later.
- [x] Pinned toolchains: `rust-toolchain.toml`, `.dvmrc`, `.nvmrc`, `packageManager`.
- [x] CI: `cargo fmt`, `clippy`, `cargo test`, `deno fmt`, `deno lint`, `deno test`.

### Phase 2: Protocol subset
- [x] TypeSpec schema for only what the slice needs:
  - `host.hello` / `host.welcome`, `ping`
  - `plugin.load`, `conn.create` / `conn.destroy` / `conn.resync`, `conn.status`
  - `state.snapshot`, `state.patch`, `event.emit`, `log`
  - `action.invoke`
- [x] TS types generated from the schema. Rust types hand-written, because `typify` failed the day-one test (ADR 0016 outcome).
- [x] Framing in Rust and TS: `u32` big-endian length + MessagePack, plus the `COVALENCE_PROTOCOL_DEBUG=json` mode.
- [x] Shared golden vectors in `protocol/fixtures/`, encoded and decoded by both sides.

### Phase 3: Core engine (Rust)
- [x] State tree: paths, values, per-value metadata (`ts`, `seq`, `quality`, `source`), `stale` on disconnect, dirty-path set.
- [x] Runtime journal: in-memory ring plus SQLite (WAL), gap-free `seq`, `cause`. Only the kinds the slice uses.
- [x] Output gate: the real pipeline. Simulate mode and term fencing work; role and show lock are pass-through stubs.
- [x] Host supervisor: spawn the Deno host, health pings, restart with backoff, `conn.resync` after restart.

### Phase 4: SDK, plugin host, Videohub plugin (TypeScript / Deno)
- [x] Minimal `definePlugin`: `ctx.state.patch`, `ctx.emit`, actions, state-level simulator hook.
- [x] Plugin host: one Deno process, one worker per connection, coarse process-level allowlist (ADR 0015).
- [x] Videohub plugin: simulator, plus real TCP via Deno sockets (host transports are deferred).
- [x] Protocol-level fake Videohub server for the plugin's own tests ([ADR 0012](../adr/0012-simulated-devices.md)).

### Phase 5: Thin server and acceptance test
- [ ] `axum` endpoints: `POST /connections`, `GET /state`, `POST /actions`, `GET /journal`, `GET /journal/{seq}/why`.
- [ ] Connections stored in the `entities` table. The project op log is deferred.
- [ ] End-to-end script for every "Done when" item, including the restart timing.

## Ordering

Phases 0–2 run in sequence. Phases 3 (Rust) and 4 (TypeScript) depend only on the protocol, so they can run in parallel. Phase 5 joins them.

## Out of scope

UI, surfaces and gestures · flows and QuickJS · project op log, undo and text export · host transports and permission enforcement · redundancy and Orbital.
