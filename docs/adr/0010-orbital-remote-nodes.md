# 0010 – Orbital remote nodes

- **Status:** Accepted. Orbital performs hardware I/O for plugins instead of running drivers ([0025](0025-device-drivers-are-plugins.md)).
- **Date:** 2026-10-18

## Context
The engine often runs in a rack room while surfaces sit at front-of-house, the producer's desk or a camera platform. Some devices are only reachable from a particular machine or network. Users also want to control software on other computers.

## Decision
**Orbital** is a small Rust binary for Windows, macOS, Linux and Raspberry Pi.
- **Surfaces:** it finds locally attached Stream Decks, MIDI controllers, X-keys and serial devices, using the same driver crate as the core (`covalence-surfaces`), and offers them to the core.
- **It's a plugin host:** it speaks the plugin protocol ([0002](0002-protocol-first-plugins.md)), so remote surfaces and remote plugins appear exactly like local ones.
- **Local computer control** (built in, permissioned):
  - launch or quit apps, check whether an app is running
  - send keystrokes and hotkeys
  - run AppleScript or PowerShell scripts the admin has registered (no arbitrary commands by default)
- **It can also host plugins** next to devices on isolated networks.
- **Discovery and security:**
  - finds cores via mDNS
  - pairs by token or by approving it in the UI
  - uses mutual TLS with a per-node identity key
- **Rendering:** the core renders images at each device's native resolution and format and sends only the controls that changed.
- **Input timing:** Orbital timestamps every input, so the core's gesture layer judges double-press and long-press by the Orbital's clock, not by network jitter.
- **Resilience:**
  - reconnects automatically, showing an "offline" icon on keys meanwhile
  - with redundancy it connects to **both** nodes, follows the leader, and can act as the failover witness ([0008](0008-redundancy-and-failover.md))

## Consequences
- The surface protocol is a subset of the plugin protocol, so there's one code path in the core.
- Browser and tablet surfaces are another kind of remote surface over the same protocol.
- Remote computer control is a powerful attack surface. It's off by default and enabled per capability, per Orbital.
