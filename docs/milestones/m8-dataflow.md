# Milestone 8: Dataflow

- **Status:** Done

Covalence gets its continuous half. Values can be computed, accumulated and sent somewhere without a flow ([dataflow §10.3](../specs/dataflow.md#103-sources-transforms-and-sinks-helpers-are-transforms)):
- **Reducers** (`toggle`, `position`, `select` …) are sandboxed transform definitions.
- **Cells** give computed and accumulated values a home in the state tree.
- **Variables** become declared entities that can be computed.
- **Sinks** keep a variable or an action parameter in step with a value.

Helpers on controls are reducers whose input defaults to their control. Layers become active from state, so shift layers and popovers arrive. The virtual surface gains an encoder and a fader, so all of this can be shown without hardware beyond M7's Stream Deck.

## Done when

- [x] A latching key: a `toggle` helper on `k3`, its background a `rules` source on the helper's value, and a flow that runs on the helper's `on` event.
- [x] A GPIO-style contact from a test plugin latches the same way, through a variable whose `value` is a `toggle` on the plugin's events. No surface is involved.
- [x] `vars.isLive` declared as `{expr: "atem.program == 3"}` (against a simulated switcher) is a read-only cell. A binding and a `state` trigger read it, and `why` on the trigger's action passes through the variable's patch to the switcher's.
- [x] A virtual encoder with a `position` helper that `follow`s `vh`'s simulated volume and has `to: {action: setVolume}`. Turning it sends rate-limited actions with the latest value. When the volume changes from elsewhere, the helper follows it and sends nothing back.
- [x] The virtual fader in motor mode: `value` bound to `vh`'s simulated volume moves it, and sends nothing back. Dragging it sends rate-limited actions, and `requested.level` and `value` are both readable. A fader input the host marks `echo: true` updates `value` and gives no `change` ([surfaces spec §11.4](../specs/surfaces.md#114-level-outputs-echoes-gesture-timing-and-per-surface-helpers)).
- [x] A virtual encoder with a `select` helper over `core` surfaces' layers and `to: {action: navigate}` pages a deck.
- [x] A shift layer (`level: over`, `when: {while: …k15.pressed}`) takes over three keys while `k15` is held and redraws within 16 ms. A latching shift reads a `toggle`.
- [x] The same shift layer with `cascade: through` binds only `label` and claims input: the keys keep the base layer's background and tally, and `why` on k3's background names the base layer. A lock layer that binds an icon and `input: block` leaves every key's look live and ignores presses. Two layers that tie on a control get `ambiguousOrder` in the editor. The lock layer binds a `label`, since `icon` isn't a property yet (core `layers` test).
- [x] Every built-in reducer has a `defineTransform` definition and a conformance suite. Any native implementation passes it.
- [x] A cycle between two computed variables is reported as `cycle` on both, and the rest of the turn goes on.
- [x] A declared variable with `persist: true` keeps its value across a server restart.
- [x] Budgets hold with cells between a change and a key: state change → redrawn key in under 16 ms with a chain of three cells.

## Known gaps

- Inputs aren't written to the journal in batches per turn, and `journal.flushInterval` isn't built ([state spec §2.4](../specs/state-and-event-log.md#24-whats-kept-and-for-how-long)).
- A reducer whose value is `undefined` (an `absolute` before any input) keeps its state on the leaf but can't journal it, since a delete op has no `reducer`, so it restarts from `init` after a failover.

## Budgets measured

In release, with cells and sandboxed reducers in the path (`crates/covalence-server/tests/dataflow_acceptance.rs`):

| Path | Budget | Development container | Apple M3, at `aa22828` |
|---|---|---|---|
| A held shift's key redrawn after the press | 16 ms | 1.8 ms median | 0.72 ms median |
| A key at the end of three chained cells redrawn after the action that moved the first | 16 ms | 4.6 ms median | 1.8 ms median |

The first measurement found 44 ms for the shift: Nagle's algorithm held small WebSocket writes for a delayed ACK, so the server's sockets set `TCP_NODELAY`.

## History

The phase-by-phase build log was trimmed once the milestone closed. It's in git: `git log -p -- docs/milestones/m8-dataflow.md`.
