# Spec: Plugin protocol

- **Status:** Draft v0.1 (protocol version `0.1.0`)
- **Related:** [ADR 0002](../adr/0002-protocol-first-plugins.md), [ADR 0003](../adr/0003-plugin-runtime-deno.md), [ADR 0010](../adr/0010-orbital-remote-nodes.md), [state and event log](state-and-event-log.md)

The protocol is the contract between the **core** and any **host**. Message shapes below use TypeScript notation for readability. The real source of truth will be the schema in `protocol/`.

---

## 1. Roles and terms

| Term | Meaning |
|---|---|
| **Core** | `covalence-core`, the single authority |
| **Host** | A process that runs plugin code and/or surfaces. Kinds: `deno-shared`, `deno-isolated`, `orbital`, and later `wasm` and `external` |
| **Plugin** | A package (`com.example.videohub@1.2.0`) |
| **Connection** | A configured instance of a plugin, with an ID (ULID) and an alias |
| **Surface** | A control device a host provides (Orbital, or plugin-provided surfaces) |

One host stream multiplexes many connections. Every message that concerns a connection carries `conn` **in the envelope** (§2.1), never in its params, so a host can route a message to the right worker without decoding `p`. Param shapes below that still show `conn` (for methods not yet in the schema) follow the same rule.

---

## 2. Transport and framing

| Link | Transport |
|---|---|
| Core ↔ local Deno host | stdio of the child process |
| Core ↔ isolated host | stdio of the child process |
| Core ↔ Orbital | TCP + mutual TLS (default port: TBD), found via mDNS `_covalence._tcp` |

- **Frame:** a `u32` big-endian length, then a MessagePack body. Maximum frame size is 16 MiB; larger payloads (images, files) are chunked.
- **Debug mode:** set `COVALENCE_PROTOCOL_DEBUG=json` for newline-delimited JSON, so the traffic is readable in a terminal.
- **Keepalive:** either side sends a `ping` request (params and result `{}`) every 2 s. Three unanswered pings mark the link dead.
- **Integers:** a peer may send any integer as a 64-bit MessagePack integer. JS decoders turn those within the safe range back into plain numbers (state spec §1.2).
- **Timestamps** (`ts`) are Unix epoch milliseconds, as a number.

### 2.1 Envelope

```ts
type Message =
  | { t: "req"; id: number; m: string; conn?: string; p: unknown }   // expects a res
  | { t: "res"; id: number; ok: true; r: unknown }
  | { t: "res"; id: number; ok: false; e: ProtocolError }
  | { t: "ntf"; m: string; conn?: string; p: unknown };              // fire-and-forget

interface ProtocolError {
  code: ErrorCode;     // see §9
  message: string;
  data?: unknown;
  retryable?: boolean;
}
```

Request IDs are unique per direction per link.

---

## 3. Session lifecycle

```
host                                core
 │── req host.hello ───────────────►│   protocol range, host kind, capabilities
 │◄────────────── res host.welcome ─│   chosen protocol version, session ID, term
 │◄──────────── req plugin.load ────│   (per plugin package)
 │── res {manifest} ───────────────►│
 │◄──────────── req conn.create ────│   (per connection)
 │── ntf conn.status(connecting) ──►│
 │── ntf state.snapshot ───────────►│
 │── ntf conn.status(ok) ──────────►│
 │        … actions / state / events …
 │◄──────────── req conn.destroy ───│
 │◄──────────── req host.shutdown ──│
```

### 3.1 `host.hello` (host → core, req)

```ts
{
  protocol: { min: "0.1.0", max: "0.1.x" },
  host: { kind: "deno-shared" | "deno-isolated" | "orbital" | "wasm" | "external",
          version: string, runtime?: string /* "deno 2.x" */ },
  node?: { id: string; name: string; publicKey: string },   // Orbital only
  capabilities: Capability[]  // "workers" | "permissions" | "surfaces" | "transports" | "simulate" | "computerControl"
}
```

The response is `host.welcome`: `{ protocol: "0.1.0", session: string, term: number, coreId: string }`.

### 3.2 `plugin.load` (core → host, req)

`{ plugin: string, version: string, bundle: string /* path, or a blob reference for Orbital */ }`

The response is `{ manifest: Manifest }`. The core checks it against the signed manifest from the package ([ADR 0013](../adr/0013-plugin-distribution.md)) and rejects any mismatch.

### 3.3 Connection lifecycle (core → host, req)

The connection ID is the envelope's `conn`. Results are `{}`.

| Method | Params |
|---|---|
| `conn.create` | `{plugin, version, alias, config, secrets, permissions, mode: "live" \| "simulate", isolate: boolean}` |
| `conn.configure` | `{config, secrets?}` (live reconfiguration; the plugin may answer `restartRequired`) |
| `conn.destroy` | `{reason}` |
| `conn.resync` | `{}`: asks for a fresh `state.snapshot` (after failover or reconnect) |

`permissions` are resolved by the core from the manifest templates and the config, for example `net: ["10.0.0.12:9990"]`. The Deno host applies them to the connection's worker.

### 3.4 `conn.status` (host → core, ntf)

`{ status: "connecting" | "ok" | "warning" | "error" | "disconnected", message?: string, detail?: unknown }`

`disconnected` and `error` make the core mark the connection's state subtree `stale`.

---

## 4. Manifest

The manifest is declared in code with the SDK's `definePlugin()`, then serialised to `covalence.plugin.json` at build time and signed.

```ts
interface Manifest {
  id: string;                    // reverse-DNS: "io.covalence.videohub"
  name: string;
  version: string;               // semver
  protocol: string;              // semver range: "^0.1"
  sdk: string;
  runtime: "deno";
  license: string;
  authors: string[];
  homepage?: string;
  categories: string[];          // "video-routing", "presentation", "camera", …
  icon?: string;

  config: {
    schema: JSONSchema;          // drives the connection form
    ui?: UiHints;                // field order, groups, help, conditional visibility
    version: number;             // config schema version (ADR 0006)
  };
  secrets?: { [key: string]: { label: string; description?: string } };

  permissions: {
    net?: string[];              // templates: "{config.host}:{config.port}", "*.local:80"
    serial?: boolean;
    udpBroadcast?: boolean;
    env?: string[];
  };
  connections: "multiple" | "single";   // policy for standby nodes (ADR 0008)

  actions: ActionDef[];
  events: EventDef[];
  state: StateSchema;            // see the state spec §1.2–1.4
  presets?: PresetDef[];         // suggested button and page templates
  surfaces?: SurfaceProviderDef; // if the plugin provides surfaces (for example a web panel)
  simulator?: { stateLevel: boolean; protocolLevel: boolean };
}

interface ActionDef {
  id: string;                    // "route"
  name: string;                  // "Set route"
  description?: string;
  params: JSONSchema;            // with x-choices: { from: "<collection path>" }
  result?: JSONSchema;
  idempotent?: boolean;          // safe to retry with the same call ID
  cancellable?: boolean;
  durationHint?: "instant" | "short" | "long";
  learn?: boolean;               // supports "learn current values" for its params
}

interface EventDef {
  id: string;                    // "timerEnded"
  name: string;
  payload?: JSONSchema;
}
```

### 4.1 Instance schema updates (host → core, ntf `schema.update`)

Some details are only known after connecting: the number of router inputs, the timer list, camera model features. The plugin can refine its schema per connection:

`{ conn, state?: StateSchemaPatch, actions?: ActionDefPatch[] }`

This only narrows or extends what the manifest declares (enum values, collection labels, feature flags). It cannot add permissions.

---

## 5. Actions

### 5.1 `action.invoke` (core → host, req)

Params (the connection is the envelope's `conn`):

```ts
{
  call: string,           // ULID; idempotency key; matches the journal's action.requested.call
  run?: string,           // flow run ID (for logs)
  term: number,           // fencing (state spec §4)
  action: string,         // "route"
  params: unknown,
  deadline?: number       // ms; the host must give up after this long
}
```

- **Success:** `{ result?: unknown }`.
- **Failure:** a `ProtocolError`, with `code: "action_failed" | "invalid_params" | "not_connected" | "timeout" | "cancelled"`.
- The host **must reject** any `term` lower than the highest it has seen (`stale_term`).

### 5.2 Progress and cancellation

| Method | Direction | Params |
|---|---|---|
| `action.progress` | host → core, ntf | `{call, progress: 0..1, message?}` |
| `action.cancel` | core → host, ntf | `{call}`. The host should abort, then reply to the original req with `cancelled`. |

### 5.3 Learn

`action.learn` (core → host, req): `{conn, action, params?}` returns the current device values as `params`. It powers "set this button to whatever the device is doing now".

---

## 6. State and events (host → core)

| Method | Type | Params |
|---|---|---|
| `state.snapshot` | ntf | `{values: {p: Path, v: Value}[], complete: true}`: replaces the connection's whole subtree |
| `state.patch` | ntf | `{ops: [{p: Path, v: Value} \| {p: Path, del: true}], ts?}` |
| `event.emit` | ntf | `{event: string, payload?: unknown, ts?}` |
| `log` | ntf | `{level: "trace" \| "debug" \| "info" \| "warn" \| "error", msg, fields?}` |

The envelope's `conn` is required, except for `log`. Paths are segment arrays ([ADR 0017](../adr/0017-wire-paths-as-segment-arrays.md)) relative to the connection root: `["timers", "countdown", "remaining"]`. The core adds the connection prefix and validates against the schema. Invalid patches are dropped and logged.

**Backpressure:**
- The SDK coalesces patches per path within a 1 ms window by default.
- When the core is overloaded, it may also coalesce, keeping the latest value per path.
- Events are never coalesced or dropped silently. If they overflow, the core sends a `log` warning and an `events_dropped` count.

### 6.1 Reading core state (host → core, req), optional

`state.get {paths}` and `state.subscribe {paths}`, plus `state.changed` (ntf, core → host). These let a plugin react to other state, for example a tally plugin that follows the program source. Gated by the manifest: `permissions.readState: ["<alias-agnostic selector>"]`.

---

## 7. Host transports (core-performed I/O)

The SDK exposes these as `ctx.tcp`, `ctx.udp`, `ctx.ws`, `ctx.http` and `ctx.serial`. The core opens the socket, checks the target against the connection's resolved `permissions`, records traffic for the inspector and for record/replay ([ADR 0012](../adr/0012-simulated-devices.md)), and handles reconnects when asked.

| Method | Direction | Params / result |
|---|---|---|
| `io.open` | host → core, req | `{conn, kind: "tcp" \| "udp" \| "ws" \| "serial", target, options: {reconnect?, backoff?, framing?: "raw" \| "line" \| "length"}}` → `{handle}` |
| `io.write` | host → core, ntf | `{handle, data: bytes}` |
| `io.data` | core → host, ntf | `{handle, data: bytes}` |
| `io.state` | core → host, ntf | `{handle, state: "open" \| "closed" \| "reconnecting" \| "error", error?}` |
| `io.close` | host → core, ntf | `{handle}` |
| `http.request` | host → core, req | `{conn, method, url, headers, body?, timeoutMs}` → `{status, headers, body}` |

Using transports is optional. A plugin that opens sockets directly through Deno is still limited by its worker permissions, but it doesn't get the traffic inspector or record/replay.

---

## 8. Surfaces (Orbital and plugin-provided)

| Method | Direction | Params |
|---|---|---|
| `surface.attach` | host → core, ntf | `{surface, model, serial?, name, controls: ControlDef[], layout: {cols, rows}}` |
| `surface.detach` | host → core, ntf | `{surface}` |
| `surface.input` | host → core, ntf | `{surface, control, kind: "down" \| "up" \| "turn" \| "touch" \| "value", value?, ts}` |
| `surface.draw` | core → host, ntf | `{surface, control, image: bytes, format: "jpeg" \| "bmp" \| "png" \| "rgb565"}` |
| `surface.led` | core → host, ntf | `{surface, control, color}` |
| `surface.brightness` | core → host, ntf | `{surface, value: 0..100}` |
| `surface.text` | core → host, ntf | `{surface, control, text}` (for controls that only show text) |

```ts
interface ControlDef {
  id: string;                       // "k0", "enc1", "strip"
  kind: "button" | "encoder" | "fader" | "touchstrip" | "lcd" | "led";
  x: number; y: number; w?: number; h?: number;     // grid position
  image?: { w: number; h: number; format: string; rotation?: 0 | 90 | 180 | 270 };
  pressable?: boolean;              // encoders that click
}
```

- **Gestures are computed in the core,** using the host's `ts`. Network jitter therefore never turns a double-press into two separate presses.
- **The core renders images** at each control's native size and format, and sends only the controls that changed.

---

## 9. Error codes

`invalid_request` · `unknown_method` · `unsupported_version` · `invalid_params` · `not_found` · `not_connected` · `permission_denied` · `action_failed` · `timeout` · `cancelled` · `stale_term` · `restart_required` · `overloaded` · `internal`

---

## 10. Versioning rules

- `0.x`: anything may change. Hosts and plugins must match the minor version.
- From `1.0`: **minor** versions add methods or optional fields and must be ignored safely by older peers. **Major** versions break compatibility, and the core supports N-1 for at least 12 months.
- Unknown `ntf` methods are ignored. Unknown `req` methods get `unknown_method`.
- Capabilities (§3.1) gate optional features. Never assume a feature from the version number alone.

---

## 11. SDK sketch (TypeScript, illustrative)

```ts
import { definePlugin, schema as s } from "@covalence-io/sdk";

export default definePlugin({
  id: "io.covalence.videohub",
  name: "Blackmagic Videohub",
  version: "0.1.0",
  connections: "multiple",
  config: s.object({
    host: s.string({ format: "hostname", label: "IP address" }),
    port: s.number({ default: 9990 }),
  }),
  permissions: { net: ["{config.host}:{config.port}"] },

  state: {
    inputs:  s.collection({ key: "index", label: "name" }, { name: s.string() }),
    outputs: s.collection({ key: "index", label: "name" }, {
      name: s.string(),
      source: s.number({ kind: "id" }),
    }),
  },

  events: { routeChanged: s.object({ output: s.number(), input: s.number() }) },

  actions: {
    route: {
      name: "Set route",
      params: s.object({
        output: s.number({ choices: { from: "outputs" } }),
        input:  s.number({ choices: { from: "inputs" } }),
      }),
      idempotent: true,
      async run(ctx, { output, input }) {
        await ctx.conn.send(`VIDEO OUTPUT ROUTING:\n${output} ${input}\n\n`);
      },
    },
  },

  async connect(ctx) {
    ctx.conn = await ctx.tcp.open(`${ctx.config.host}:${ctx.config.port}`, {
      framing: "line", reconnect: true,
    });
    ctx.conn.on("block", (block) => {
      // parse the device's status blocks → ctx.state.patch(...) and ctx.emit("routeChanged", …)
    });
  },

  simulator: {
    // a state-level simulator: answers actions with plausible state
    init: (sim) => sim.state.set({ inputs: { 0: { name: "Cam 1" } }, outputs: { 0: { name: "PGM", source: 0 } } }),
    actions: { route: (sim, { output, input }) => sim.state.patch(`outputs[${output}].source`, input) },
  },
});
```

---

## 12. Open questions

1. ~~Schema tool~~: TypeSpec → JSON Schema. See [ADR 0016](../adr/0016-protocol-schema-tooling.md).
2. ~~Path format on the wire~~: segment arrays. See [ADR 0017](../adr/0017-wire-paths-as-segment-arrays.md).
3. Transferring Orbital plugin bundles: should the core push the package over the link, or should the Orbital fetch it from the registry?
4. Should plugins be able to define **custom gestures or controls**, such as a jog wheel's acceleration curve, or does the core own all gesture logic?
5. Port number and mDNS TXT records for Orbital discovery.
