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
  directNetwork?: boolean;       // (M6) opens sockets itself instead of through host transports (§7, ADR 0025)
  devices?: DeviceMatch[];       // (M6) hardware this plugin drives, matched on hotplug (§7.1)
  connections: "multiple" | "single";   // policy for standby nodes (ADR 0008)

  actions: ActionDef[];
  events: EventDef[];
  state: StateSchema;            // see the state spec §1.2–1.4
  presets?: PresetDef[];         // suggested button and layer templates
  implements?: InterfaceImpls;   // proposed: interfaces such as "camera.ptz@1" (ADR 0019)
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
| `state.patch` | ntf | `{ops: [{p: Path, v: Value} \| {p: Path, del: true}], call?, ts?}` |
| `event.emit` | ntf | `{event: string, payload?: unknown, call?, ts?}` |
| `log` | ntf | `{level: "trace" \| "debug" \| "info" \| "warn" \| "error", msg, fields?}` |

The envelope's `conn` is required, except for `log`. Paths are segment arrays ([ADR 0017](../adr/0017-wire-paths-as-segment-arrays.md)) relative to the connection root: `["timers", "countdown", "remaining"]`. The core adds the connection prefix and validates against the schema. Invalid patches are dropped and logged.

**Causation:** `call` is the `action.invoke` call ID (§5.1) of the action that produced the change or event. The SDK sets it for writes and events made through the context an action's `run` (or its simulator handler) receives, while that call is running; one patch never mixes ops from different calls, or from a call and outside one. Changes from device traffic outside an action (including a device's echo of a command an action sent) carry no `call`. While the call is in flight on that connection, the core journals the entry with `cause` → the call's `action.requested` entry (state spec §2.2); a `call` it doesn't know, or one belonging to another connection, is ignored. The host must send these before the call's response, and the core handles a host's messages in the order it sent them.

**Backpressure:**
- The SDK coalesces patches per path within a 1 ms window by default.
- When the core is overloaded, it may also coalesce, keeping the latest value per path.
- Events are never coalesced or dropped silently. If they overflow, the core sends a `log` warning and an `events_dropped` count.

### 6.1 Reading core state (host → core, req), optional

`state.get {paths}` and `state.subscribe {paths}`, plus `state.changed` (ntf, core → host). These let a plugin react to other state, for example a tally plugin that follows the program source. Gated by the manifest: `permissions.readState: ["<alias-agnostic selector>"]`.

---

## 7. Host transports (core-performed I/O)

The SDK exposes these as `ctx.tcp`, `ctx.udp`, `ctx.ws`, `ctx.http`, `ctx.serial`, `ctx.hid` and `ctx.midi`. The core opens the socket or device, checks the target against the connection's resolved `permissions`, records traffic for the inspector and for record/replay ([ADR 0012](../adr/0012-simulated-devices.md)), and handles reconnects when asked. Transports are byte-level: what the bytes mean is the plugin's business ([ADR 0025](../adr/0025-device-drivers-are-plugins.md)).

| Method | Direction | Params / result |
|---|---|---|
| `io.open` | host → core, req | `{conn, kind: "tcp" \| "udp" \| "ws" \| "serial" \| "hid" \| "midi", target, options: {reconnect?, backoff?, framing?: "raw" \| "line" \| "length", baud?}}` → `{handle}`. **(M6)** For `serial`, `hid` and `midi`, `target` is a device ID (§7.1) |
| `io.write` | host → core, ntf | `{handle, data: bytes}`. For `hid`, an output report whose first byte is the report ID; for `midi`, one or more MIDI messages |
| `io.data` | core → host, ntf | `{handle, data: bytes}`. For `hid`, an input report; for `midi`, one message |
| `io.feature` | host → core, req | **(M6)** `hid` only: `{handle, op: "get", reportId, length}` → `{data}`, or `{handle, op: "send", data}` → `{}` |
| `io.state` | core → host, ntf | `{handle, state: "open" \| "closed" \| "reconnecting" \| "error", error?}`. **(M6)** A device handle closes with `error: "unplugged"` and opens again when the same device returns |
| `io.close` | host → core, ntf | `{handle}` |
| `http.request` | host → core, req | `{conn, method, url, headers, body?, timeoutMs}` → `{status, headers, body}` |

- **Hardware goes through the core.** Deno has no HID, MIDI or serial of its own, and plugins don't use native add-ons for them. `usb` (interfaces, bulk, interrupt and control transfers) comes later.
- **One connection holds a device at a time.** A second `io.open` of the same device is refused with `busy`.
- **(M6) Device handles outlive a plugin host restart.** The core keeps the device open, drops its data while no host is there, and gives the restarted connection the same device on its next `io.open`.
- **Network transports are optional.** A plugin that opens sockets itself declares `directNetwork: true` (§4). It's still limited by its worker's permissions once they're enforced ([ADR 0015](../adr/0015-defer-plugin-permission-enforcement.md)), but its traffic isn't in the inspector or record/replay, and an Orbital can't route it. **(M6)** The SDK's `ctx.tcp` returns an object shaped like `Deno.Conn`, and `ctx.net.socket()` one shaped like a `node:net` `Socket`, so libraries that accept a socket can use the core's.

### 7.1 Devices (M6)

The core enumerates HID, serial and MIDI devices, watches for hotplug, and matches them against installed plugins' `devices`.

```ts
interface DeviceMatch {
  kind: "hid" | "serial" | "midi";
  vendorId?: number;
  productId?: number | number[];
  usagePage?: number;            // hid
  name?: string;                 // glob over the product or MIDI port name
  autoCreate?: boolean;          // create a connection without asking (surfaces)
}

interface DeviceInfo {
  id: string;                    // stable: from kind, vendor, product and serial number where there is one, else the port path
  kind: "hid" | "serial" | "midi";
  vendorId?: number;
  productId?: number;
  serial?: string;
  manufacturer?: string;
  product?: string;
  path: string;                  // OS path or port name, for display
  usagePage?: number;
  usage?: number;
}
```

| Method | Direction | Params / result |
|---|---|---|
| `device.list` | host → core, req | `{conn, kind?}` → `DeviceInfo[]`: devices this plugin's `devices` match, for a picker |

- **A connection names its device in its config,** as a string field with `format: "device"`. The connection form offers matching devices that no connection holds.
- **When a matching device appears that no connection holds,** the core offers to create a connection for it in the UI, with the device in its config, or creates one when the match says `autoCreate`. A device several plugins match is offered once, with a choice.
- **OS limits are reported as `io.state` errors** the UI can explain: exclusive access refused (macOS keyboards and mice, another app holding the device), missing permission (Linux udev rules), no driver (Windows WinUSB).

---

## 8. Surfaces (Orbital and plugin-provided)

A surface is a two-way device ([ADR 0024](../adr/0024-surfaces-as-two-way-devices.md), [surfaces spec](surfaces.md)). The host reports its controls' input, which the core keeps as state and derives events from, and the core sets their outputs. **(Now)** marks the M5 Phase 5 reshaping; **(M6)**, [M6](../milestones/m6-surfaces-from-plugins.md); **(Later)**, what has no milestone yet. The web surface carries these notifications today (surfaces §8). **(M6)** The plugin host carries them, with the envelope's `conn` naming the connection that provides the surface: a hardware surface is a plugin that reads its device through `ctx.hid` (§7) and translates ([ADR 0025](../adr/0025-device-drivers-are-plugins.md)). Orbitals carry them **(Later)**.

| Method | Direction | Params |
|---|---|---|
| `surface.attach` | host → core, ntf | `{surface, model, serial?, name, controls: ControlDef[], outputs?: {brightness?: boolean}}`. **(Now)** No `layout`. **(M6)** From the plugin host, `surface` is the host's own key for the device, and the core matches the attach to a surface entity by `model` and `serial` (surfaces §1) |
| `surface.detach` | host → core, ntf | `{surface}` |
| `surface.input` | host → core, ntf | `{surface, control, kind: "down" \| "up" \| "turn" \| "touch" \| "value", value?, ts, echo?}` |
| `surface.draw` | core → host, ntf | `{surface, control, image: bytes, format: "jpeg" \| "bmp" \| "png" \| "rgb565"}`, for `image` outputs |
| `surface.led` | core → host, ntf | `{surface, control, color}`, for `led` outputs **(Later)** |
| `surface.level` | core → host, ntf | `{surface, control, value: 0..1}`, for `level` outputs: motor faders, encoder rings, meters **(Later)** |
| `surface.text` | core → host, ntf | `{surface, control, text}`, for `text` outputs **(Later)** |
| `surface.brightness` | core → host, ntf | `{surface, value: 0..100}`, when the attach declares `brightness` **(Later)** |

```ts
interface ControlDef {                // (Now) capabilities replace `kind`, `image` and `pressable`
  id: string;                         // unique per surface: "k0", "dial1", "pad.36"
  label?: string;
  at: { x: number; y: number; w?: number; h?: number };    // (Now) required: grid cells; controls don't overlap
  inputs?: InputCap[];
  outputs?: OutputCap[];
  suggest?: Helper[];                 // (Later) default helpers for this control (bindings §5)
}

type InputCap =
  | { kind: "momentary"; velocity?: boolean }   // down, up
  | { kind: "relative" }                        // turn
  | { kind: "absolute"; axes?: 1 | 2 }          // value
  | { kind: "touch" };                          // touch

type OutputCap =
  | { kind: "image"; w: number; h: number; format: string; rotation?: 0 | 90 | 180 | 270 }
  | { kind: "led"; rgb?: boolean }
  | { kind: "level"; motor?: boolean }
  | { kind: "text"; chars?: number; lines?: number };
```

| Input `kind` | Capability | `value` | Input state (surfaces §3) |
|---|---|---|---|
| `down` | `momentary` | Velocity, 0 to 1, if the capability has `velocity` | `pressed: true`, `velocity` |
| `up` | `momentary` | none | `pressed: false` |
| `turn` | `relative` | Signed detents since the last `turn` | `detents` changes by `value` |
| `value` | `absolute` | 0 to 1, or `[x, y]` with `axes: 2` | `value` |
| `touch` | `touch` | 1 when touched, 0 when released | `touched` |

- **The SDK has presets** for common controls: `button`, `encoder`, `fader`, `touchstrip`, `lcd` and `led` build the capability lists that M5's `kind` stood for. A device with no physical layout (GPIO pins) still gets an `at`: the SDK places its controls in a row.
- **Input state, events and gestures are computed in the core,** using the host's `ts` (surfaces §4). Network jitter therefore never turns a double-press into two separate presses. Hosts send what happened, not what it means: a MIDI host normalises its controller's relative encoding into signed detents, and a 14-bit fader into 0 to 1.
- **A host can't define helpers.** It can `suggest` defaults for a control, such as a jog wheel's acceleration.
- **`echo: true`** **(Later)** marks input the device sent because the core moved it, such as a motor fader reaching its requested position (surfaces §4.4). A host that can't tell leaves it out, and the core decides from timing.
- **Input that doesn't match a control's capabilities** (a `turn` on a control with no `relative` input) is dropped and logged, rate-limited.
- **The core renders images** at each `image` output's native size and format, and sends only the outputs that changed.

---

## 9. Error codes

`invalid_request` · `unknown_method` · `unsupported_version` · `invalid_params` · `not_found` · `not_connected` · `permission_denied` · `action_failed` · `timeout` · `cancelled` · `stale_term` · `busy` · `restart_required` · `overloaded` · `internal`

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
4. ~~Should plugins be able to define **custom gestures or controls**, such as a jog wheel's acceleration curve, or does the core own all gesture logic?~~ Decided by [ADR 0024](../adr/0024-surfaces-as-two-way-devices.md): controls are any set of capabilities, their input is state with events derived in the core, the core owns helpers, and a host can `suggest` defaults (§8).
5. Port number and mDNS TXT records for Orbital discovery.
