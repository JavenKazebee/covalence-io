# 0012 – Simulated devices built into the SDK

- **Status:** Accepted
- **Date:** 2026-10-18

## Context
Shows are programmed before the hardware is available. Plugins need tests that don't require a lab. Bugs often show up only with a real device's timing and quirks.

## Decision
The SDK and core provide five things:
1. **State-level simulation:** a connection runs in `mode: simulate`. The plugin's simulator hook accepts actions, publishes believable state and emits events. Any plugin can add this cheaply.
2. **Protocol-level simulators:** an SDK toolkit for fake device servers (a fake Videohub TCP server, a fake ProPresenter API). The real plugin code runs against them, so they double as the plugin's automated test suite.
3. **Record and replay:** host transports pass every byte through the core, so a real device session can be recorded and replayed as a simulator.
4. **Virtual surfaces:** every supported surface model (Stream Deck with dials and touch strip, Neo, XL, MIDI controllers) rendered in the UI.
5. **Rehearsal mode:** the whole project runs against simulators, with outputs suppressed at the output gate ([0007](0007-event-sourced-core.md)), much like offline programming on a lighting console.

## Consequences
- The registry can show a "has simulator" badge and the CI status of each plugin.
- Recordings can contain sensitive data, so the UI needs a redaction step before sharing one.
