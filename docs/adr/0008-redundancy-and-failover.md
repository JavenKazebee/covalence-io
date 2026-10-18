# 0008 – Redundancy and failover in three levels

- **Status:** Accepted. Design it into v1; build it after the MVP.
- **Date:** 2026-10-18

## Context
Live shows can't stop. A small show shouldn't pay for complexity it doesn't need, and a big one wants a hot backup that takes over automatically.

## Decision
Three opt-in levels:

| Level | What the backup does | Takeover |
|---|---|---|
| **1. Config mirror** | Keeps an up-to-date copy of the project | Manual |
| **2. Warm standby** | Mirrors the project and live state, and keeps allowed device connections open. Sends no outputs. | One click |
| **3. Hot standby** | As level 2, also mirrors in-flight runs and their checkpoints | Automatic when the heartbeat is lost |

Mechanics:
- **Leader-based replication:** the active node streams both logs ([0007](0007-event-sourced-core.md)) to the standby. The standby **applies** them; it doesn't recompute.
- **Split brain prevention:**
  - Automatic failover needs a quorum from a **witness**. Any Orbital or a small witness process can act as one.
  - Without a witness, takeover is manual.
  - A **leadership term** number is attached to every output, so outputs from an old leader are rejected.
- **Device connection policy** in the plugin manifest:
  - `connections: multiple` — the standby connects and observes.
  - `connections: single` — the standby stays disconnected until it's promoted.
- **Runs in progress at failover** follow a per-flow policy: `abort` (default) | `resume` (from the last checkpointed step) | `restart`.
- **Surfaces survive failover** only if they're attached through Orbitals, which connect to both nodes and follow the leader.
- **Time:** both nodes must agree on the time source ([0014](0014-time-sources-and-scheduling.md)).

## Consequences
- Simple setups pay nothing: one node, no witness.
- Plugins must declare their connection policy, and actions must be safe to retry where they declare `idempotent: true`. The call ID is the deduplication key.
- Surfaces plugged directly into the active node's USB are lost if that machine dies. The docs must say this clearly.
