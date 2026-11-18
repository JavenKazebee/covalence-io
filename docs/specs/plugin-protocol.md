# Spec: Plugin protocol

- **Status:** Draft v0.1 (protocol version `0.1.0`)
- **Related:** [state and event log](state-and-event-log.md), [dataflow](dataflow.md), [composition](composition.md)

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

The response is `{ manifest: Manifest }`. The core checks it against the signed manifest from the package ([§13.10](#1310-distribution-registry-signing-offline-install-pinning-not-built)) and rejects any mismatch.

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
    version: number;             // config schema version (state spec §7.2)
  };
  secrets?: { [key: string]: { label: string; description?: string } };

  permissions: {
    net?: string[];              // templates: "{config.host}:{config.port}", "*.local:80"
    serial?: boolean;
    udpBroadcast?: boolean;
    env?: string[];
  };
  directNetwork?: boolean;       // (M7) opens sockets itself instead of through host transports (§7, §13.6)
  devices?: DeviceMatch[];       // (M7) hardware this plugin drives, matched on hotplug (§7.1)
  connections: "multiple" | "single";   // policy for standby nodes (state spec §7.4)

  actions: ActionDef[];
  events: EventDef[];
  state: StateSchema;            // see the state spec §1.2–1.4
  presets?: PresetDef[];         // suggested button and layer templates; (M9) replaced by `components`
  components?: ComponentDef[];   // (M9) reusable controls: look, helpers, flows, params, parts (composition §8)
  transforms?: string[];         // (M9) transform definition modules, run in the core's sandbox (dataflow §4.4)
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
  effect?: "internal" | "device";   // (M6) default "device"; simulate mode suppresses only "device" (core actions §4)
}

interface EventDef {
  id: string;                    // "timerEnded"
  name: string;
  payload?: JSONSchema;
  at?: string;                   // (M6) a collection path the event happens under: "outputs" (§6)
}
```

- **(M9) `components`** are content with no code ([§13.6](#136-device-drivers-are-plugins-the-core-performs-hardware-io)): the `Component` shape of [composition §8](composition.md), with IDs namespaced by the plugin. They replace `presets`.
- **(M9) `transforms`** are paths to modules in the package, each a `defineTransform` ([dataflow §4.4](dataflow.md)). The core loads them into its own sandbox, never into the plugin host, and names them `<plugin id>/<id>`. A module that fails to load or to pass its declared types is skipped with an error on the plugin; the rest of the plugin is unaffected.

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
| `event.emit` | ntf | `{event: string, payload?: unknown, at?: Path, call?, ts?}`. **(M6)** `at` is the node the event happened at, relative to the connection root (`["outputs", 3]`), so it's addressed `vh.outputs[3]:routeChanged` ([dataflow §3](dataflow.md)). Absent, it's the root |
| `log` | ntf | `{level: "trace" \| "debug" \| "info" \| "warn" \| "error", msg, fields?}` |

The envelope's `conn` is required, except for `log`. Paths are segment arrays ([§13.3](#133-paths-on-the-wire-are-segment-arrays)) relative to the connection root: `["timers", "countdown", "remaining"]`. The core adds the connection prefix and validates against the schema. Invalid patches are dropped and logged.

**Causation:** `call` is the `action.invoke` call ID (§5.1) of the action that produced the change or event. The SDK sets it for writes and events made through the context an action's `run` (or its simulator handler) receives, while that call is running; one patch never mixes ops from different calls, or from a call and outside one. Changes from device traffic outside an action (including a device's echo of a command an action sent) carry no `call`. While the call is in flight on that connection, the core journals the entry with `cause` → the call's `action.requested` entry (state spec §2.2); a `call` it doesn't know, or one belonging to another connection, is ignored. The host must send these before the call's response, and the core handles a host's messages in the order it sent them.

**Backpressure:**
- The SDK coalesces patches per path within a 1 ms window by default.
- When the core is overloaded, it may also coalesce, keeping the latest value per path.
- Events are never coalesced or dropped silently. If they overflow, the core sends a `log` warning and an `events_dropped` count.

### 6.1 Reading core state (host → core, req), optional

`state.get {paths}` and `state.subscribe {paths}`, plus `state.changed` (ntf, core → host). These let a plugin react to other state, for example a tally plugin that follows the program source. Gated by the manifest: `permissions.readState: ["<alias-agnostic selector>"]`.

---

## 7. Host transports (core-performed I/O)

The SDK exposes these as `ctx.tcp`, `ctx.udp`, `ctx.ws`, `ctx.http`, `ctx.serial`, `ctx.hid` and `ctx.midi`. The core opens the socket or device, checks the target against the connection's resolved `permissions`, records traffic for the inspector and for record/replay ([overview: simulated devices](overview.md#simulated-devices)), and handles reconnects when asked. Transports are byte-level: what the bytes mean is the plugin's business ([§13.6](#136-device-drivers-are-plugins-the-core-performs-hardware-io)).

| Method | Direction | Params / result |
|---|---|---|
| `io.open` | host → core, req | `{kind: "tcp" \| "udp" \| "ws" \| "serial" \| "hid" \| "midi", target, options: {reconnect?, backoff?, framing?: "raw" \| "line" \| "length", baud?}}` → `{handle}`. **(M7)** For `serial`, `hid` and `midi`, `target` is a device ID (§7.1) |
| `io.write` | host → core, ntf | `{handle, data: bytes}`. For `hid`, an output report whose first byte is the report ID; for `midi`, one or more MIDI messages |
| `io.data` | core → host, ntf | `{handle, data: bytes, ts?}`. For `hid`, an input report; for `midi`, one message. **(M7)** `ts` is when the core read it, in ms since the epoch on the core's clock: a surface plugin sends it as its input's `ts` |
| `io.feature` | host → core, req | **(M7)** `hid` only: `{handle, op: "get", reportId, length}` → `{data}`, or `{handle, op: "send", data}` → `{}` |
| `io.state` | core → host, ntf | `{handle, state: "open" \| "closed" \| "reconnecting" \| "error", error?}`. **(M7)** A device handle closes with `error: "unplugged"` and opens again when the same device returns |
| `io.close` | host → core, ntf | `{handle}` |
| `http.request` | host → core, req | `{conn, method, url, headers, body?, timeoutMs}` → `{status, headers, body}` |

- **The connection is the envelope's `conn`,** as for every other method from a connection, and a connection can use only its own handles. **Bytes** (`data`, `image`) are MessagePack `bin`, and arrays of byte values in the JSON debug format.
- **(M7) `tcp`:** `target` is `host:port`. `io.open` answers once the first attempt to connect has succeeded, or fails with `not_connected`; with `reconnect`, it answers either way and the handle goes `reconnecting` (backoff from `initialMs`, 500 ms, doubling to `maxMs`, 10 s). Writes while it's away are dropped. `framing`: `raw` sends what each read gave; `line` one `io.data` per line, its `\n` kept; `length` one per message after a 4-byte big-endian length. Sockets close when the plugin host goes. A handle's `io.state` and `io.data` follow `io.open`'s response.
- **(M7) The core keeps each handle's recent traffic** (inspector, [overview spec: simulated devices](overview.md#simulated-devices)): what it read and wrote and its state changes, at `GET /connections/{id}/traffic`.
- **Hardware goes through the core.** Deno has no HID, MIDI or serial of its own, and plugins don't use native add-ons for them. `usb` (interfaces, bulk, interrupt and control transfers) comes later.
- **One connection holds a device at a time.** A second `io.open` of the same device is refused with `busy`.
- **(M7) Device handles outlive a plugin host restart.** The core keeps the device open, drops its data while no host is there, and gives the restarted connection the same device on its next `io.open`.
- **Network transports are optional.** A plugin that opens sockets itself declares `directNetwork: true` (§4). It's still limited by its worker's permissions once they're enforced ([§13.5](#135-permissions-are-declared-now-enforced-later)), but its traffic isn't in the inspector or record/replay, and an Orbital can't route it. **(M7)** The SDK's `ctx.tcp` returns an object shaped like `Deno.Conn`, and `ctx.net.socket()` one shaped like a `node:net` `Socket`, so libraries that accept a socket can use the core's.

### 7.1 Devices (M7)

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
| `device.list` | host → core, req | `{kind?}` → `DeviceInfo[]`: devices this plugin's `devices` match, for a picker |

- **IDs** are `<kind>:<vendor>:<product>:<serial>` in lowercase hex, `hid:0fd9:0080:AL12K1A01234`, so a device keeps its ID in any port. A device without a serial number is `<kind>@<path>`, which holds only while it stays in that port. HID interfaces of one device that share a serial number add `/<usage page>.<usage>`, `…:AL12K1A01234/ff00.0001`, and their path if that still isn't unique.
- **A connection names its device in its config,** as a string field with `format: "device"` (the first top-level one). The connection form offers matching devices that no connection holds. A connection holds a device when its config names it or it has it open.
- **When a matching device appears that no connection holds,** the core offers to create a connection for it in the UI, with the device in its config, or creates one when the match says `autoCreate`. A device several plugins match is offered once, with a choice. It's created without asking only when exactly one of the plugins that match it says `autoCreate`; the connection is `live`.
- **A handle exists while its device is away.** `io.open` of a device that isn't plugged in, or that the OS refuses, gives a handle all the same, whose first `io.state` says why (`closed` with `unplugged`, or `error`); it opens when the device is plugged in (again). Only `busy` refuses the request.
- **OS limits are reported as `io.state` errors** the UI can explain: exclusive access refused (macOS keyboards and mice, another app holding the device), missing permission (Linux udev rules), no driver (Windows WinUSB).

---

## 8. Surfaces (Orbital and plugin-provided)

A surface is a two-way device ([surfaces §11.2](surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state), [surfaces spec](surfaces.md)). The host reports its controls' input, which the core keeps as state and derives events from, and the core sets their outputs. **(Now)** marks the M5 Phase 5 reshaping; **(M7)**, [M7](../milestones/m7-surfaces-from-plugins.md); **(Later)**, what has no milestone yet. The web surface carries these notifications today (surfaces §8). **(M7)** The plugin host carries them, with the envelope's `conn` naming the connection that provides the surface: a hardware surface is a plugin that reads its device through `ctx.hid` (§7) and translates ([§13.6](#136-device-drivers-are-plugins-the-core-performs-hardware-io)). Orbitals carry them **(Later)**.

| Method | Direction | Params |
|---|---|---|
| `surface.attach` | host → core, ntf | `{surface, model, serial?, name, controls: ControlDef[], outputs?: {brightness?: boolean}}`. **(Now)** No `layout`. **(M7)** From the plugin host, `surface` is the host's own key for the device, and the core matches the attach to a surface entity by `model` and `serial` (surfaces §1), or by `model` and the connection when there's no serial, creating one on the first attach with `provider` set to the connection |
| `surface.detach` | host → core, ntf | `{surface}` |
| `surface.input` | host → core, ntf | `{surface, control, kind: "down" \| "up" \| "turn" \| "touch" \| "value", value?, ts, echo?}` |
| `surface.draw` | core → host, ntf | `{surface, control, image: bytes, format: "jpeg" \| "bmp" \| "png" \| "rgb565" \| "rgb"}`, for `image` outputs. **(M7)** `rgb` is raw 8-bit RGB, rows top to bottom, for a driver that packs images itself |
| `surface.led` | core → host, ntf | `{surface, control, color}`, for `led` outputs **(Later)** |
| `surface.level` | core → host, ntf | `{surface, control, value: 0..1}`, for `level` outputs: motor faders, encoder rings, meters. **(M8)** The SDK's `onLevel`; a plugin reports its motor's own movement with `echo` ([surfaces spec §11.4](surfaces.md#114-level-outputs-echoes-gesture-timing-and-per-surface-helpers)) |
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
- **A host can't run helpers.** It can `suggest` defaults for a control, such as a jog wheel's acceleration. **(M9)** A plugin can also ship transforms (§4), which the core runs in its own sandbox, never in the host ([dataflow §10.3](dataflow.md#103-sources-transforms-and-sinks-helpers-are-transforms)).
- **`echo: true`** **(Later)** marks input the device sent because the core moved it, such as a motor fader reaching its requested position (surfaces §4.4). A host that can't tell leaves it out, and the core decides from timing.
- **Input that doesn't match a control's capabilities** (a `turn` on a control with no `relative` input) is dropped and logged, rate-limited.
- **The core renders images** at each `image` output's native size and format, and sends only the outputs that changed. **(M7)** It draws `png`, `jpeg` and `rgb`, turned by `rotation` degrees clockwise; an attach with an image output it can't draw is refused. From the plugin host, `surface.draw`'s `surface` is the connection's own key.
- **(M7) A connection that stops** (destroyed, or its plugin host lost) detaches its surfaces, which releases their input (surfaces §4.1). The device keeps what it shows until it's attached again.

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

1. ~~Schema tool~~: TypeSpec → JSON Schema (§13.2).
2. ~~Path format on the wire~~: segment arrays (§13.3).
3. Transferring Orbital plugin bundles: should the core push the package over the link, or should the Orbital fetch it from the registry?
4. ~~Should plugins be able to define **custom gestures or controls**, such as a jog wheel's acceleration curve, or does the core own all gesture logic?~~ Decided by [surfaces §11.2](surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state): controls are any set of capabilities, their input is state with events derived in the core, the core owns helpers, and a host can `suggest` defaults (§8).
5. Port number and mDNS TXT records for Orbital discovery.

---

## 13. Decisions

Why the protocol and the plugin host are the way they are, and what was rejected. Keep these notes when the sections above change: they stop old alternatives coming back.

### 13.1 Protocol first
- **Decided:** the plugin protocol is versioned and language-neutral, and it's the contract: every SDK and host implements it. The schema in `protocol/` is the source of truth. Frames are length-prefixed MessagePack, with a newline-delimited JSON debug mode (§2). The TypeScript SDK is the first-class SDK. Other languages implement the same protocol.
- **Why:** the device ecosystem decides whether the project succeeds, and plugins have to run in different hosts (the shared Deno host, isolated processes, Orbitals, later WASM or other languages) and outlive any one runtime. Orbital is then just another host. The cost: the protocol is a public API, so a breaking change needs a major version and a deprecation window (§10).
- **Rejected:**
  - *A runtime-specific plugin API* (Companion's model): faster to start, but ties the ecosystem to one runtime.
  - *gRPC/protobuf:* heavier, and awkward for dynamic JSON-shaped state and schemas.
  - *Companion module compatibility as the foundation:* modules are built around buttons, with no first-class events and loosely typed variables. A compatibility adapter may come later, as a separate, lower-tier layer.

### 13.2 Schema tooling: TypeSpec → JSON Schema
- **Decided:** the schema is written in TypeSpec and emitted as JSON Schema, which is committed so other-language SDKs never need TypeSpec. TypeScript types are generated (`json-schema-to-typescript`). **Rust types in `covalence-protocol` are written by hand**, and golden vectors in `protocol/fixtures/` that both test suites decode, re-encode and compare are the contract between the two. Opaque payloads that are themselves JSON Schema (`config.schema`, action `params`) are plain JSON values.
- **Why hand-written Rust:** tested on 2026-10-18 with `typify` 0.8.0 on the §2.1 envelope. It dropped `const` (`t: "req"` became `String`), made the union `#[serde(untagged)]` so a notification with a stray `id` decoded as a request, and turned opaque payloads into `serde_json::Value`, which can't hold MessagePack binary. Fixing that meant changing the wire format to suit the tool. `json-schema-to-typescript` produced clean discriminated unions.
- **Rejected:** hand-written JSON Schema (verbose, error-prone); Rust-first (`serde` + `schemars`) or TypeScript-first (zod), which make one language the source of truth; protobuf (§13.1).

### 13.3 Paths on the wire are segment arrays
- **Decided:** on the wire and in storage a path is `(string | number)[]`, such as `["outputs", "SDI 4", "source"]`. The text form (`vh.outputs["SDI 4"].source`) is only for people: expressions, the UI, logs and docs, and the core converts at those edges. The SDK accepts text paths and converts before sending.
- **Why:** state patches are the highest-volume traffic (target 10,000 updates/s). No parsing or escaping on the hot path, no quoting bugs with keys like `"SDI 4"` or `"a.b"` across two languages, and the store already keeps references in this form. Frames are slightly larger.
- **Rejected:** dotted strings with bracket syntax on the wire.

### 13.4 One shared Deno host, a worker per connection
- **Decided:** one Deno process hosts every plugin by default. Each connection runs in its own worker, with its own event loop, and can be terminated alone. An **Isolate** toggle per connection moves it into its own process. The core supervises every host: health pings, restart with backoff, memory and CPU limits, all visible in the UI. The SDK has no runtime-specific code.
- **Why:** plugin work is mostly waiting on the network, so V8 is far faster than needed. The real cost is memory per process (30–60 MB each in Companion's process-per-connection model). Companion 3 moved modules out of process because in 2.x one bad module could freeze the app; workers give that isolation for crashes and hangs. The risk: a native crash or out-of-memory in the shared process takes down every non-isolated connection. Mitigations: fast restart with state resynced, and automatically isolating a connection that crashes the host repeatedly.
- **Rejected:**
  - *Process per connection:* best isolation, highest memory.
  - *Process per plugin type:* the fallback if per-worker permissions never work (§13.5).
  - *Node:* best npm and native-add-on support, weaker sandbox. *Bun:* no sandbox, gaps in Node compatibility.
  - *V8 or QuickJS embedded in the core:* a native crash would take down the engine.
  - *WASM components:* excellent isolation, but immature networking and no npm ecosystem. A possible future host type.

### 13.5 Permissions are declared now, enforced later
- **Decided:** `permissions` stay in the manifest and in `conn.create`, and the UI shows them, but **per-connection permissions aren't enforced yet**: no per-worker narrowing and no core checks on host transports. The shared host runs with a coarse process-level allowlist instead (`packages/plugin-host/README.md`): network allowed, reads limited to the plugin directories, no `--allow-run`, `--allow-ffi` or `--allow-sys`.
- **Why:** permissions protect against plugins you don't trust, and until the registry takes third-party plugins every plugin is first-party. Per-worker narrowing relies on Deno's `deno.permissions` worker option, behind `--unstable-worker-options`. What matters in a show, crash and hang isolation, the workers give anyway. Declaring permissions from the start avoids a breaking protocol change later.
- **Release blocker:** enforcement must land before the registry opens to third-party plugins. Then: confirm per-worker narrowing works on the current Deno, measure memory with 50 connections and restart-to-restored (target under 3 s), check a worker in a busy loop can be terminated alone, and enforce both layers. If narrowing doesn't work, rely on the core-enforced transports and make process-per-plugin-type the isolation unit. Once enforced, a worker without `directNetwork` gets no network permission at all.
- **Rejected:** enforcing from the start (guards a threat that doesn't exist yet, on an unstable API); dropping permissions from the manifest (adding them later breaks the protocol).

### 13.6 Device drivers are plugins; the core performs hardware I/O
- **Decided:** everything that knows a device's protocol (HID report layouts, key image formats, MIDI mappings, serial framing) is in a plugin. The core has no built-in drivers. It keeps input state, gesture timing, events, rendering, layers and helpers (surfaces spec): a surface plugin translates, it doesn't interpret. The core owns enumeration, hotplug and the byte-level `hid`, `serial` and `midi` transports (`usb` later), holds each device for one connection, reconnects across unplug and replug, and keeps hardware open across a plugin host restart (§7.1). An Orbital performs hardware I/O the same way, so a plugin doesn't know where its device is.
- **Why:** with drivers in the core, every new surface model or USB gadget would need a core release. Core I/O gives what a plugin can't: no native add-ons (Deno's least-tested path), one owner per device, hotplug matching in one place, devices that survive a host restart, and Orbitals opening devices on their own machine. The cost: surface input and key images cross the plugin host (device → core → host → core), an IPC round trip inside the 5 ms key-down budget, and the core needs a device layer on every OS with clear errors for macOS exclusive access, WinUSB and udev rules.
- **Devices that need native code** (DeckLink, AJA, NDI) would use a native add-on in an isolated connection, or a helper process that speaks the protocol. Not designed yet.
- **Rejected:** drivers in the core; plugins opening hardware with native add-ons (every plugin scans USB, and a host restart closes every device); a WASM host for drivers (deferred, not rejected).

### 13.7 Network I/O is the plugin's choice
- **Decided:** host transports (`ctx.tcp`, `ctx.udp`, `ctx.ws`, `ctx.http`) are optional. A plugin that opens its own sockets declares `directNetwork: true`, and the UI marks it as not inspectable. First-party plugins use host transports, and the SDK's socket shapes (§7) let libraries that take a socket use them unchanged.
- **Why:** the core performing I/O gives the inspector, record/replay, reconnects and enforceable permissions. But most network device libraries open their own sockets with `node:net` or `fetch`, and requiring core transports would mean forking them, with every protocol the core lacks (multicast, mDNS, HTTP/2, SRT, custom TLS) a dead end. The cost: `directNetwork` traffic isn't in the inspector or record/replay, and an Orbital can't route it.
- **Rejected:** requiring host transports for all network I/O.

### 13.8 What plugins can define
- **Now:** connections (actions, state, events), surfaces, and content with no code (presets, layer templates).
- **Later:** time sources, flow templates, and pure functions such as transforms, key widgets and expression libraries. Pure functions run in the core's QuickJS sandbox, not the plugin host, so they fit the render budget (dataflow §4.4).
- **Never:** flow step kinds, scheduling, gesture rules, helpers or layer resolution. Cancellation, `why`, replay and timing depend on these being the core's.

### 13.9 Dynamic targets (proposed)
A show often needs one layer that controls *any* camera, with a row of keys to choose which. In Companion each action is fixed to one connection, so users copy the page per camera.

- **A target** is `{conn, key?}`: a connection, and optionally one item of a collection it exposes, since one connection can drive many devices. A step's target is fixed or a source (dataflow §1) with a **constraint**: a plugin ID, a composite template (composition §6.5), or one of the core's interfaces. The editor uses it to type-check parameters, and the runtime checks it. A target can be stored in state (`kind: "target"`, with an optional constraint), in ID form, so renaming an alias never breaks it. In expressions `target(t)` is a read-only view of its state, narrowed to `key`, with `.alias`, `.name`, `.plugin`, `.connected` and `.supports(action)`. A target that resolves to nothing or fails the constraint fails the step with `no_target`, and nothing is dispatched. The journal records the resolved target (`key?`).
- **Only the core implements interfaces.** `surface@1` and `flow@1` ([core actions §2](core-actions.md)) name the actions of the core's own collections, so "the selected deck" has a type. Plugins don't declare interfaces, and there's no shared set of device interfaces.
- **Devices from different plugins meet in a composite template** (composition §6.5). The template is the project's own contract ("Camera": tally, `recallPreset`, `speed`), each instance binds real devices, and where the plugins differ the template bridges them in its state expressions and owned flows, for example by branching on `target(params.ptz).plugin`.
- **Mismatches between a control and a device are the user's to bridge, in the open:** an encoder driving a camera that only takes positions goes through a `position` reducer on the binding, where `why` can follow it.
- **Rejected:**
  - *Fixed targets:* force copying the layer per camera.
  - *Duck typing on action names:* names and parameters don't line up across plugins, and nothing checks they mean the same.
  - *An aggregator plugin per show:* hand-written each time, and hides the real target from the journal. (Composites do this in the open: composition §6.)
  - *Device interfaces* (`ptz.speed@1`, `tally@1`, `recorder@1` …, kept in `protocol/interfaces/` and implemented by plugins through manifest `implements`, with `at` for collections, `range` conversions and composites that `delegate` to them): devices of one kind don't work the same way, so the set keeps splitting into capability-sized pieces, each needing mappings, conversions and conformance checks in the SDK and the core, and every interface is a promise to plugin authors that's hard to take back. Most rigs use one plugin per kind of device, where plugin-constrained targets already stop the copying, and composite templates cover mixed rigs. Revisit when projects keep building the same composite over the same plugins, or when third-party plugins need to act on "any switcher" with no project in between. If interfaces come back, these stay rejected: *one interface per kind of device* (it has to pick one control model, and every device that works another way is emulated inside its plugin, out of sight), *general conversions in manifest mappings* (a second dataflow language), and *interfaces defined by plugins* (several incompatible "camera" interfaces).

### 13.10 Distribution: registry, signing, offline install, pinning (not built)
- **Decided:** a `.cvplugin` archive holds the manifest, bundled code with every dependency vendored, the licence, simulator files and a checksum. It's signed by the publisher and countersigned by the registry, and the core checks both before installing (Sigstore or minisign/ed25519, to choose). The registry is a public index with versions, declared permissions, and badges for "has simulator", CI status and verified publisher; start with a static signed index file before a service. Install offline by dropping a `.cvplugin` into the UI, or inside a `.covalence` bundle. **Projects pin exact plugin versions**: updates are proposed, never automatic, and the review shows the changelog and any permission changes. Several versions can be installed side by side. Install shows the plugin's permissions in words ("can connect to {config.host}:{config.port}").
- **Why:** show machines are often offline, an update must never change behaviour mid-show, and plugins come from strangers. Vendoring makes packages larger but reproducible offline.
