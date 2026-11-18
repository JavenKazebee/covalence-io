# Milestone 2: First UI slice

- **Status:** Done
- **Started:** 2026-10-18

Put a face on M1. The core serves a Vue 3 UI ([overview: the UI](../specs/overview.md#the-ui-vue-3-served-by-the-core-with-an-optional-tauri-shell)) that creates connections, shows live state, runs actions and walks the journal. Live values arrive over a WebSocket that subscribes only to the paths on screen.

## Done when

- [x] `cargo run -p covalence-server` serves the built UI at `/`; the HTTP API stays where it is.
- [x] A Videohub connection can be created from a form generated from the plugin manifest's config schema.
- [x] The State tab shows `vh.outputs[*].source` updating live as routes change, with no reload, and shows `stale` while the plugin host is down.
- [x] An action form generated from the manifest's action params runs `vh.route`, and the result appears in the journal view.
- [x] Clicking a journal entry shows its `why` chain.
- [x] A browser smoke test of the above runs in CI against a real server in simulate mode.

## History

The phase-by-phase build log was trimmed once the milestone closed. It's in git: `git log -p -- docs/milestones/m2-first-ui.md`.
