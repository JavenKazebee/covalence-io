# 0025 – Device drivers are plugins; the core performs hardware I/O, and network I/O is the plugin's choice

- **Status:** Proposed
- **Date:** 2026-10-22
- **Amends:** [0001](0001-rust-core.md) (built-in device drivers in the core), [0002](0002-protocol-first-plugins.md) (point 5: host transports), [0003](0003-plugin-runtime-deno.md) (hardware access belongs in the core's built-in drivers), [0010](0010-orbital-remote-nodes.md) (Orbital finds devices with the `covalence-surfaces` driver crate). The plugin protocol, the Deno host and the core's surface model ([0024](0024-surfaces-as-two-way-devices.md)) stand.
- **Specs:** [plugin protocol §4, §7, §8](../specs/plugin-protocol.md), [surfaces](../specs/surfaces.md)
- **Milestone:** [M6: Surfaces from plugins](../milestones/m6-surfaces-from-plugins.md)

## Context
- **The architecture puts device drivers in the core:** "Built-in drivers: Stream Deck HID · MIDI · OSC · TCP/UDP/HTTP/WS · Serial", in a `covalence-surfaces` crate shared with Orbital. Every new surface model, MIDI controller or USB gadget would then need a core release.
- **The reason was the runtime, not ownership.** Deno can reach HID and USB only through native npm add-ons (`node-hid`, `usb`, `serialport`), which are its least-tested path ([0003](0003-plugin-runtime-deno.md)).
- **Existing TypeScript device libraries fall into three groups:**
  1. a pluggable I/O backend: `@elgato-stream-deck/core` keeps device logic apart from its `node-hid` and WebHID backends
  2. opening their own sockets with `node:net` or `fetch`: most network device libraries, and the first-party Videohub plugin (`Deno.connect`)
  3. native add-ons for hardware
- **[0002](0002-protocol-first-plugins.md) has the core perform transport I/O** so it can enforce permissions, record traffic for the inspector and for record/replay, and reconnect. None of the host transports is built yet ([M1](../milestones/m1-walking-skeleton.md) deferred them).
- **For network devices, requiring core transports costs a lot:** most group 2 libraries would need forking, and every protocol the core doesn't offer (multicast, mDNS, HTTP/2, SRT, custom TLS) is a dead end until it does.
- **For hardware, core I/O gives things a plugin can't:** no native add-ons, one owner for each device, hotplug matching in one place, a device that stays open across a plugin host restart, and Orbitals opening devices on their own machine.
- **WASM components were considered as a second plugin host** and set aside: TypeScript on Deno stays the only plugin runtime for now. [0002](0002-protocol-first-plugins.md) still leaves room for other hosts later.

## Decision

### Device drivers are plugins
- **Everything that knows a device's protocol is in a plugin:** HID report layouts, key image formats, MIDI mappings, serial framing. The core has no built-in device drivers, and `covalence-surfaces` isn't built.
- **The core keeps what [0024](0024-surfaces-as-two-way-devices.md) gave it:** input state, gesture timing, events, rendering, layers and helpers. A surface plugin translates; it doesn't interpret.
- **A Stream Deck is the first such plugin,** `io.covalence.streamdeck`, built on `@elgato-stream-deck/core` with a backend over `ctx.hid`.

### The core performs hardware I/O
- **New host transports:** `hid` and `midi`, beside `serial`; `usb` (claim an interface, bulk, interrupt and control transfers) later. They are byte-level and know nothing about any device. HID adds feature reports, which Stream Decks and many others use.
- **The core owns enumeration and hotplug.** A manifest declares the devices it handles (`devices`: kind, vendor and product IDs, HID usage page, MIDI port name). When a matching device appears and no connection holds it, the core offers a connection with that plugin, or creates one if the match says `autoCreate`. A connection names its device by a stable ID, built from the serial number where the device has one.
- **One connection holds a device at a time.** A second open is refused with `busy`.
- **Unplug and replug are reconnects:** the handle goes `closed` with `unplugged`, and opens again by itself when the same device returns. The plugin doesn't reopen it.
- **A plugin host restart doesn't close hardware.** The core keeps the handle open, drops data while no host is there, and hands the same handle to the restarted connection. Surfaces the connection provided detach and release their input as on any detach (surfaces §4.1), and reattach when it's back. The device keeps showing its last images meanwhile.
- **An Orbital performs hardware I/O the same way** for devices plugged into it, so a plugin doesn't know where its device is.

### Network I/O is the plugin's choice
- **Plugins may open sockets themselves.** `ctx.tcp`, `ctx.udp`, `ctx.ws` and `ctx.http` are optional, as [0002](0002-protocol-first-plugins.md) already allowed, but they're no longer the expected path for third-party libraries.
- **The SDK makes the core path easy:** `ctx.tcp` returns an object shaped like `Deno.Conn`, and `ctx.net.socket()` one shaped like a `node:net` `Socket`, so a library that accepts a socket goes through the core unchanged.
- **First-party plugins use host transports.** The Videohub plugin moves off `Deno.connect` first.
- **Bypassing is declared.** A manifest that opens its own sockets says `directNetwork: true`, and lists what it reaches in `permissions.net` as before. The UI marks those connections as not inspectable. When permissions are enforced ([0015](0015-defer-plugin-permission-enforcement.md)), a worker without `directNetwork` gets no network permission at all, and one with it gets `permissions.net`.

### What plugins can define
- **Now:** connections (actions, state, events), surfaces, and content with no code (presets and layer templates).
- **Later:** interfaces ([0019](0019-dynamic-action-targets-and-interfaces.md)), time sources ([0014](0014-time-sources-and-scheduling.md)), flow templates, and pure functions such as key widgets and expression libraries. Pure functions would run in the core's QuickJS sandbox ([0021](0021-expressions-as-sandboxed-snippets.md)), not in the plugin host, so they fit the render budget.
- **Never:** flow step kinds, scheduling, gesture rules, helpers or layer resolution. These are the core's semantics: cancellation, `why`, replay and timing depend on them.

### Devices that need native code
- **A device that needs a vendor SDK or kernel driver** (DeckLink, AJA, NDI) uses a native add-on in an isolated connection, or a helper process that speaks the plugin protocol. Neither is designed yet.

## Consequences
- A new surface model, MIDI controller or USB gadget with a standard class needs only a plugin. An ESP32 that shows up as USB serial, HID or MIDI is a plugin of plain TypeScript.
- Libraries with a pluggable backend (group 1) are used whole. Group 2 libraries work as they are, outside the inspector unless they take a socket. Group 3 libraries are used through their protocol layer, not their native one.
- Surface input crosses the plugin host: device → core → host → core. This adds an IPC round trip to the 5 ms key-down budget, which M6 measures.
- Key images cross it too: the core renders, the host packs the image into reports, and the core writes them. At 15 keys of about 10 KB this is small, and frames are binary.
- The core needs a device layer (HID, serial, MIDI, hotplug) on every platform, with clear errors for OS limits: macOS's exclusive-access rules, Windows' WinUSB, Linux udev rules.
- Traffic from `directNetwork` plugins isn't in the inspector or record/replay, and Orbitals can't route it.
- Network permission enforcement gets simpler: most workers need no network permission once first-party plugins use host transports.

## Alternatives considered
- **Device drivers in the core,** as planned: every device needs a core release, and the plugin ecosystem can't add surfaces.
- **Plugins open hardware themselves with native add-ons:** Deno's weakest path, every plugin scans USB on its own, and a host restart closes every device.
- **Require host transports for all network I/O:** the inspector and record/replay would cover everything, but most network libraries would need forking.
- **A WASM component host for drivers:** strong isolation and no IPC hop, but a second runtime and SDK. Deferred, not rejected.
- **Plugins define their own flow steps or helpers:** rejected for the reasons in [0024](0024-surfaces-as-two-way-devices.md).
