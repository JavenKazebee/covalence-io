# @covalence-io/plugin-host

The `deno-shared` plugin host
([ADR 0003](../../docs/adr/0003-plugin-runtime-deno.md)). One Deno process runs
every non-isolated connection, each in its own worker. It speaks the
[plugin protocol](../../docs/specs/plugin-protocol.md) with the core on stdio.

## Launching (for the core's host supervisor)

Run it with this coarse, process-level allowlist
([ADR 0015](../../docs/adr/0015-defer-plugin-permission-enforcement.md)).
`<root>` is the repository (later: the install directory).

```sh
deno run --no-prompt \
  --config=<root>/deno.json \
  --allow-read=<root>/packages,<root>/plugins \
  --allow-net \
  --allow-env=COVALENCE_PROTOCOL_DEBUG \
  <root>/packages/plugin-host/main.ts
```

| Flag                                          | Why                                                                                                                   |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `--no-prompt`                                 | A missing permission fails instead of blocking on a TTY prompt.                                                       |
| `--config`                                    | Resolves the workspace packages (`@covalence-io/*`) wherever the core's working directory is.                         |
| `--allow-read=<root>/packages,<root>/plugins` | The host imports `worker.ts` and the plugin bundles. Add any other plugin directory the core passes to `plugin.load`. |
| `--allow-net`                                 | Plugins open device sockets directly until host transports land. Not narrowed per connection yet.                     |
| `--allow-env=COVALENCE_PROTOCOL_DEBUG`        | Set it to `json` for newline-delimited JSON instead of MessagePack.                                                   |

Not granted: `--allow-write`, `--allow-run`, `--allow-ffi`, `--allow-sys`, and
other environment variables. Workers inherit these permissions; `conn.create`'s
`permissions` are not applied yet (ADR 0015).

`plugin.load`'s `bundle` is an absolute path or a URL. For Milestone 1 it is the
plugin's `mod.ts` (for example `<root>/plugins/videohub/mod.ts`); its default
export must be a `definePlugin()` result.

Stdout carries only protocol frames. The host's own diagnostics go to stderr;
plugin `console.*` output becomes `log` notifications.

## Behaviour

- Sends `host.hello` (`kind: "deno-shared"`, capabilities `workers` and
  `simulate`) on start, and takes the initial term from `host.welcome`.
- Pings the core every 2 s; after three unanswered pings it exits with code 1.
  It exits with 0 on stdin EOF or `host.shutdown`.
- Routes requests to workers by the envelope's `conn` and passes params through.
  The main thread reads only `action.invoke`'s `term` (host-wide fencing: lower
  than the highest seen → `stale_term`) and `deadline` (→ `timeout`, and the
  call's `signal` is aborted).
- A request for a connection whose `conn.create` (or whose plugin's
  `plugin.load`) is still in progress waits for it.
- A worker that crashes is terminated: its connection reports `conn.status`
  `error`, its requests fail with `not_connected`, and other connections are
  unaffected. The core recovers it with `conn.destroy` and `conn.create`.

## Tests

`host_test.ts` drives the real process with a fake core (`fake_core.ts`),
launched with the flags above.
