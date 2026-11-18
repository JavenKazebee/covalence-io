# Milestone 1: Walking skeleton

- **Status:** Done
- **Started:** 2026-10-18

Route a Blackmagic Videohub in simulate mode, end to end: API → core → output gate → Deno plugin host → plugin → state patch → journal. Every layer of the architecture is touched; no layer is built out.

## Done when

- [x] `POST /actions` with `vh.route {output: 1, input: 3}` journals `action.requested` → `action.dispatched` → `action.result`.
- [x] The plugin's `state.patch` updates `vh.outputs[1].source`.
- [x] `GET /journal/{seq}/why` walks the cause chain back to the API call.
- [x] Killing the Deno host process makes the core restart it and resync state in under 3 s.
- [x] All of the above runs in CI with no hardware.

## Running it

```bash
cargo run -p covalence-server -- --data data
```

```bash
curl -s -XPOST localhost:7680/connections -H 'content-type: application/json' -d '{"plugin":"io.covalence.videohub","alias":"vh","config":{"host":"10.0.0.12"}}'
```

```bash
curl -s -XPOST localhost:7680/actions -H 'content-type: application/json' -d '{"action":"vh.route","params":{"output":1,"input":3}}'
```

```bash
curl -sg 'localhost:7680/state?path=vh.outputs[1]'
```

The acceptance test is `crates/covalence-server/tests/e2e.rs` (needs `deno` on the PATH).

## Known gaps, carried forward

- In live mode a Videohub route's `state.patch` arrives with the device's echo, outside the action, so it has no `cause`. Only changes made while an action runs (the simulator, or optimistic writes in `run`) link to it. So the flow loop guard can't see live loops; since M4 an `event` `filter` that excludes the flow's own change breaks them ([flow spec](../specs/flows.md) §8.6).
- In simulate mode the simulated device lives in the plugin worker, so a host crash resets it to its initial state.

## History

The phase-by-phase build log was trimmed once the milestone closed. It's in git: `git log -p -- docs/milestones/m1-walking-skeleton.md`.
