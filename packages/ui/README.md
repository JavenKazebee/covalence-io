# ui

The Covalence IO web UI: Vue 3, TypeScript, Vite and Pinia ([ADR 0005](../../docs/adr/0005-web-ui-vue-tauri-shell.md)).

```sh
cargo run -p covalence-server   # API on :7680
pnpm --filter ui dev            # UI on :5173, proxying the API and /ws to :7680
```

`pnpm --filter ui` also runs `type-check`, `lint`, `test` and `build`.

Live state comes over one shared WebSocket (`src/live/client.ts`, messages in the
[state spec](../../docs/specs/state-and-event-log.md) §1.6) into a Pinia store
(`src/stores/live.ts`). Components call `useLiveState(path)`, which subscribes on
mount and unsubscribes on unmount.
