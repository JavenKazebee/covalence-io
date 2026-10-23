# 0003 – Plugins share one Deno runtime, with per-connection isolation

- **Status:** Accepted, pending the validation spike below. Permission enforcement and the spike are deferred by [0015](0015-defer-plugin-permission-enforcement.md). Hardware access is the core's device I/O for plugins, not built-in drivers ([0025](0025-device-drivers-are-plugins.md)).
- **Date:** 2026-10-18

## Context
- Most plugin work is waiting on the network, so V8 is far faster than needed.
- The real cost is memory per process: 30–60 MB each in Companion's process-per-connection model.
- Plugins come from a public registry and run on show machines, so sandboxing matters.
- Companion 3 moved modules out of process because, in 2.x, one bad module could crash or freeze the whole app. It also wanted modules to release independently of the app.

## Decision
- **One shared Deno process** hosts all plugins by default.
- **Each connection runs in its own Deno worker,** with its own event loop. The worker can be terminated on its own and is started with **only the permissions its manifest and config allow.**
- **An "Isolate" toggle per connection** moves it into its own Deno process with its own permissions. Use it for flaky, heavy or untrusted plugins.
- **The core supervises every host:** health pings, restart with backoff, memory and CPU limits, all visible in the UI.
- **Permissions are enforced twice:** by Deno (process or worker permissions) and by the core for host transports (see [0002](0002-protocol-first-plugins.md)).
- **The SDK has no runtime-specific code.** A `runtime: node` escape hatch is possible later but not in v1.

## Consequences
- Memory stays low, and startup with many connections is fast.
- A native crash, out-of-memory or hard exit in the shared process takes down **every non-isolated connection**. Mitigations:
  - fast host restart, with state resynced from snapshots
  - automatic isolation of a connection that crashes the host repeatedly
- Deno's native-add-on support is less tested than Node's. Hardware access belongs in the core's built-in drivers or host transports, so this should rarely matter.

## Validation spike (one week) – acceptance criteria
1. Port three real device libraries (one WebSocket, one raw TCP, one that uses an npm library heavily) and run them under tight permissions.
2. Confirm **per-worker permission narrowing** works on the current Deno release, and check whether it still needs an unstable flag.
3. Measure memory per worker and with 50 connections.
4. Measure host restart → all connections restored (target < 3 s).
5. Confirm a worker stuck in a busy loop can be terminated without affecting the other workers.

If (2) fails, keep per-plugin sandboxing by relying on core-enforced transports, and make process-per-plugin-type the default isolation unit.

## Alternatives considered
- **Process per connection** (Companion): best isolation, highest memory.
- **Process per plugin type:** a good middle ground, and the fallback described above.
- **Node:** best npm and native-add-on compatibility, but a weaker sandbox.
- **Bun:** fast, but no sandbox and gaps in Node compatibility.
- **V8 or QuickJS embedded in the core:** fast, but a native crash takes down the engine.
- **WASM components:** excellent isolation, but immature networking and no npm ecosystem. Planned as a future host type.
