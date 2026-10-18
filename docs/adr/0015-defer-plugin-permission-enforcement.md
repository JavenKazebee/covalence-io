# 0015 – Defer plugin permission enforcement until third-party plugins

- **Status:** Accepted
- **Date:** 2026-10-18
- **Amends:** [0003](0003-plugin-runtime-deno.md) (permission enforcement and the validation spike only; the shared runtime and worker-per-connection model stand)

## Context
- [0003](0003-plugin-runtime-deno.md) enforces each connection's manifest permissions twice: by narrowing the Deno worker's permissions, and in the core for host transports. It made the decision conditional on a validation spike.
- Per-worker permission narrowing relies on Deno's `Worker` `deno.permissions` option, which has sat behind `--unstable-worker-options`.
- Permissions protect against plugins you don't trust. Until the registry ([0013](0013-plugin-distribution.md)) accepts third-party plugins, every plugin is first-party.
- What matters during a show is crash and hang isolation, which the worker-per-connection model gives without any permission work.

## Decision
- **Keep `permissions` in the manifest and in `conn.create`.** Plugins declare what they need from the start, and the UI can show it. Adding a required protocol field later would be a breaking change; ignoring one now costs nothing.
- **Don't enforce per-connection permissions yet.** No per-worker narrowing, and no core checks on host transports.
- **Run the shared Deno host with a coarse process-level allowlist** instead: network allowed; file reads limited to the plugin directory; no `--allow-run`, `--allow-ffi` or `--allow-sys` beyond what the host itself needs.
- **Postpone the 0003 validation spike** until enforcement is built. Memory per worker and restart time are still measured, as part of [Milestone 1](../milestones/m1-walking-skeleton.md).

## Consequences
- No dependency on an unstable Deno API in v1.
- Until enforcement lands, a plugin can reach any network host. This is acceptable for first-party plugins only.
- **Enforcement is a release blocker for opening the registry to third-party plugins.** At that point, run the 0003 spike and implement both layers, or fall back to process-per-plugin-type as 0003 describes.

## Alternatives considered
- **Enforce from the start, as 0003 planned:** protects against a threat that doesn't exist yet, and ties v1 to an unstable API.
- **Drop permissions from the manifest:** saves a little now, but makes adding them later a breaking protocol change.
