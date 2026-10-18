# Milestone 2: First UI slice

- **Status:** In progress
- **Started:** 2026-10-18

Put a face on M1. The core serves a Vue 3 UI ([ADR 0005](../adr/0005-web-ui-vue-tauri-shell.md)) that creates connections, shows live state, runs actions and walks the journal. Live values arrive over a WebSocket that subscribes only to the paths on screen.

## Done when

- [ ] `cargo run -p covalence-server` serves the built UI at `/`; the HTTP API stays where it is.
- [ ] A Videohub connection can be created from a form generated from the plugin manifest's config schema.
- [ ] The State tab shows `vh.outputs[*].source` updating live as routes change, with no reload, and shows `stale` while the plugin host is down.
- [ ] An action form generated from the manifest's action params runs `vh.route`, and the result appears in the journal view.
- [ ] Clicking a journal entry shows its `why` chain.
- [ ] A browser smoke test of the above runs in CI against a real server in simulate mode.

## Phases

### Phase 1: WebSocket API (Rust)
- [x] `GET /ws`: JSON messages. Clients `subscribe` / `unsubscribe` to state path prefixes and opt in to the journal tail; the server sends a `snapshot`, then coalesced `patch` batches, `journal` entries and `host` status.
- [x] Dirty sets filtered per client; a lagged client gets a fresh snapshot instead of a gap.
- [x] REST `GET /state` and the WebSocket share one leaf serializer.
- [x] Messages documented in the [state spec](../specs/state-and-event-log.md) §1.6.

### Phase 2: UI scaffold
- [x] `packages/ui`: Vue 3 + TypeScript + Vite + Pinia, in the pnpm workspace.
- [x] Vite dev proxy for the API and `/ws` to `:7680`.
- [x] CI: type-check, lint, build.

### Phase 3: Live state client
- [x] One shared WebSocket feeding a Pinia store (path → leaf).
- [x] Components subscribe on mount and unsubscribe on unmount.
- [x] Reconnect with backoff, resubscribing afterwards.

### Phase 4: Screens
- [ ] Connections: list and create.
- [ ] State: tree with quality badges.
- [ ] Actions: manifest-driven form.
- [ ] Journal: live tail plus a `why` drawer.
- [ ] Host status in the header.

### Phase 5: Serving and acceptance
- [ ] The server serves the built UI (`--ui <dir>`, SPA fallback).
- [ ] Browser smoke test for every "Done when" item, in CI.

## Ordering

Phase 1 and Phases 2–3 can run in parallel once the WebSocket messages are written down. Phase 4 needs Phase 3. Phase 5 joins them.

## Out of scope

Login and roles (the server binds to localhost) · Tauri shell · Surfaces, Flows and Timeline tabs · Cmd+K · embedding the UI in the binary · tuning for 10,000 updates/s (measure only).
