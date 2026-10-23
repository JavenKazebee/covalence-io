# Milestone 7: Surfaces from plugins

- **Status:** Planned, after [M6](m6-primitives.md)

A real Stream Deck drives Covalence, through a plugin. The core gets a device layer: it enumerates HID and serial devices, watches hotplug, matches them to plugins' manifests and performs their I/O, so a plugin of plain TypeScript can drive hardware with no native add-ons ([ADR 0025](../adr/0025-device-drivers-are-plugins.md)). The plugin host carries the surface protocol ([plugin protocol §8](../specs/plugin-protocol.md)), and `io.covalence.streamdeck` translates between the deck's reports and M5's surface model. Network transports arrive too, and the Videohub plugin moves onto them. MIDI, Orbitals and controls beyond keys stay out.

## Done when

- [ ] Plugging in a Stream Deck MK.2 (15 keys) is noticed by the core, matched to the Stream Deck plugin's `devices`, and gets a connection and a surface without asking (`autoCreate`). The surface shows its start layer, and its keys fire event triggers on `{layer, controls}` ([M6](m6-primitives.md)).
- [ ] Unplugging it detaches the surface and releases held keys. Plugging the same deck back in reattaches the same surface entity, matched by `model` and `serial`, showing the layer it showed last.
- [ ] `why` on an action fired from the deck walks back through `trigger.matched` and the key's `event` entry to the `state.patch` on `surface.<slug>.controls.k0.pressed`, with the host's `deviceTs`.
- [ ] The Stream Deck plugin is TypeScript on `@elgato-stream-deck/core` with a backend over `ctx.hid`. No native add-ons, and no device-specific code in the core.
- [ ] Restarting the plugin host doesn't close the deck. The surface detaches and reattaches within 3 s, and the keys keep their last images meanwhile.
- [ ] Budgets hold with the plugin host in the path, measured from the device layer: key down → first action dispatched in under 5 ms, and state change → key image written to the device in under 16 ms.
- [ ] A serial device (an ESP32 over USB CDC, or a fake one in tests) can be picked in a connection's form and read and written through `ctx.serial`.
- [ ] The Videohub plugin uses `ctx.tcp`, and a connection's traffic can be viewed in the UI. A plugin with `directNetwork: true` is marked as not inspectable.
- [ ] A Rust end-to-end test covers the above in CI against a fake HID Stream Deck and a fake serial device, with no hardware.

## Phases

### Phase 0: Decisions and docs
- [x] **[ADR 0025](../adr/0025-device-drivers-are-plugins.md):** device drivers are plugins; the core performs hardware I/O; network I/O is the plugin's choice. Protocol shapes in [plugin protocol §4, §7, §7.1 and §8](../specs/plugin-protocol.md), and `provider` and serial matching in [surfaces §1](../specs/surfaces.md).
- [ ] **Spike: `@elgato-stream-deck/core` under Deno.** It loads from npm, its HID device interface can be implemented over a stub, and it runs with no native add-on in the dependency tree. If it can't, write the MK.2's protocol in the plugin directly; it's small.
- [ ] **Choose the Rust crates** for HID (`hidapi` or `nusb`-based), serial (`serialport`) and hotplug. `hidapi` has no hotplug; decide between polling enumeration (once a second) and OS notifications.
- [ ] **Protocol schema:** the §7 additions, §7.1 and the §8 `conn` in TypeSpec, with fixtures.
- [ ] Accept ADR 0025 once the spike passes. (ADR 0024 is accepted in [M6](m6-primitives.md).)

### Phase 1: The core's device layer
- [ ] **`covalence-devices`:** enumerate HID and serial devices, watch hotplug, and give each a stable `DeviceInfo.id` (§7.1).
- [ ] **Matching:** installed plugins' `devices` against present devices; offers for unclaimed matches, and connections for `autoCreate`.
- [ ] **Handles:** one connection per device (`busy`), unplug and replug as `io.state`, and handles kept open across a plugin host restart.
- [ ] **A fake backend** for tests: plug, unplug, input reports in, output and feature reports recorded.
- [ ] OS errors that the UI can explain (§7.1).

### Phase 2: Transports in the protocol and the SDK
- [ ] `io.open`, `io.write`, `io.data`, `io.feature`, `io.state` and `io.close` for `hid`, `serial` and `tcp`, and `device.list`.
- [ ] SDK: `ctx.hid`, `ctx.serial` and `ctx.tcp`, with `ctx.tcp` shaped like `Deno.Conn` and `ctx.net.socket()` like a `node:net` `Socket`. The `device` config format and its picker.
- [ ] A connection's traffic is kept in a ring buffer per handle and served at `GET /connections/{id}/traffic`.

### Phase 3: Surfaces over the plugin host
- [ ] The core accepts §8 notifications from the plugin host, with `conn` as the provider, through the transport-independent surface module M5 built. Images go as binary frames.
- [ ] Surface entities are created on first attach and matched by `model` and `serial` after that; `provider` is set.
- [ ] A connection that stops (deleted, disabled, host lost) detaches its surfaces, releasing input as on any detach (surfaces §4.1).
- [ ] SDK: `ctx.surfaces.attach(...)` with the button presets, giving input to send and draws to receive.

### Phase 4: The Stream Deck plugin
- [ ] `plugins/streamdeck`: `devices` for Elgato's vendor ID and the MK.2's product IDs, `autoCreate: true`.
- [ ] Attaches 15 `momentary` + `image` controls placed with `at`, in the device's native image size, format and rotation; turns key reports into `surface.input` with the report's time as `ts`; writes `surface.draw` images to the device.
- [ ] Other models the library supports work if they only have keys, but only the MK.2 is checked by hand.
- [ ] A protocol-level simulator of the MK.2 for tests. The UI's virtual deck is unchanged.

### Phase 5: Network transports and the Videohub plugin
- [ ] The Videohub plugin moves from `Deno.connect` to `ctx.tcp`, with reconnects from the core.
- [ ] `directNetwork` in the manifest, shown on the connection.

### Phase 6: UI
- [ ] Found devices: what's plugged in, which plugin and connection hold each, and offers for unclaimed matches.
- [ ] A connection's traffic view, and the "not inspectable" mark for `directNetwork`.
- [ ] A surface shows its provider, and whether it's attached.

### Phase 7: Acceptance
- [ ] The Rust end-to-end test against fake devices runs in CI.
- [ ] Checked by hand with a real MK.2: plug in, press a key that routes the Videohub, see the bound key redraw, unplug and replug, restart the plugin host.
- [ ] Budgets measured with the plugin host in the path, including the round trip M5 left unmeasured.
- [ ] Carried forward from M5: what a surface shows is kept across a server restart, so a surface without a start layer comes back showing the layer it showed last.

## Ordering

Phase 0 comes first. Phases 1 and 2 can run side by side once the protocol shapes are in TypeSpec. Phase 3 needs Phase 2's transport. Phase 4 needs Phases 1–3, but can start against the fake backend after the spike. Phase 5 needs only Phase 2's `ctx.tcp`. Phase 6 follows the APIs it shows, and Phase 7 joins them.

## Out of scope

MIDI and `ctx.midi` · `usb`, BLE and GPIO · Stream Deck models with dials, touch strips or LEDs, and `led`, `level` and `text` outputs and brightness · Orbitals and routing traffic through them · `ctx.udp`, `ctx.ws` and `ctx.http` · record/replay of traffic · permission enforcement ([ADR 0015](../adr/0015-defer-plugin-permission-enforcement.md)) · native add-ons and helper processes for vendor SDKs · a WASM plugin host · plugin-provided pure functions, interfaces and time sources · surface groups · everything left out of M5.
