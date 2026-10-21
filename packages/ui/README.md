# ui

The Covalence IO web UI: Vue 3, TypeScript, Vite and Pinia ([ADR 0005](../../docs/adr/0005-web-ui-vue-tauri-shell.md)).

```sh
cargo run -p covalence-server   # API on :7680
pnpm --filter ui dev            # UI on :5173, proxying the API and /ws to :7680
```

`pnpm --filter ui` also runs `type-check`, `lint`, `test` and `build`. Once built,
`cargo run -p covalence-server` serves the UI at `/` from `packages/ui/dist`
(`--ui <dir>` to serve another build, `--no-ui` for the API only).

The browser smoke test (`e2e/`, Playwright) covers every "Done when" item in
the [M2 milestone](../../docs/milestones/m2-first-ui.md). It starts the real
server on :7690 with a fresh data directory:

```sh
pnpm --filter ui build
pnpm --filter ui exec playwright install chromium   # once
pnpm --filter ui smoke
```

Live state comes over one shared WebSocket (`src/live/client.ts`, messages in the
[state spec](../../docs/specs/state-and-event-log.md) §1.6) into a Pinia store
(`src/stores/live.ts`). Components call `useLiveState(path)`, which subscribes on
mount and unsubscribes on unmount.

Screens (`src/views/`) live in the URL hash (`#/state/vh.outputs`, `#/journal/42`),
so UI routes never shadow the HTTP API's paths. Connection config and action
params are forms generated from the plugin manifest's JSON Schemas
(`src/forms/schema.ts`); `x-choices: {from: "inputs"}` fills a dropdown from the
connection's live state. The journal follows the live tail, fetches the backlog
over `GET /journal`, and refetches any gap.
