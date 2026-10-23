# 0019 – Dynamic action targets and plugin interfaces

- **Status:** Proposed
- **Amended by:** [0026](0026-sources-and-selectors.md) (`TargetSpec` becomes a `Source<Target>`), [0027](0027-events-addressed-by-path.md) (interface events), [0030](0030-composite-connections.md) (composites implement interfaces)
- **Date:** 2026-10-18
- **Amends:** [0004](0004-javascript-expressions-and-scripts.md). Its dependency-tracking section was accepted separately, as part of [0021](0021-expressions-as-sandboxed-snippets.md)
- **Specs:** [flows](../specs/flows.md), [bindings](../specs/bindings.md), [plugin protocol](../specs/plugin-protocol.md)

## Context
- A common show need is one page that controls *any* camera, with a row of buttons at the top to choose which. In Companion this can't be built: each action and feedback is fixed to one connection when it's created, so users duplicate the page once per camera.
- In the current specs, an action belongs to one plugin (`ActionDef` in the manifest) and a step names one connection. The same limit would apply.
- Two cases need solving:
  1. **Same plugin:** several cameras of the same make. The target connection has to be chosen at run time.
  2. **Different plugins:** PTZOptics, Panasonic and BirdDog cameras on one show. Their actions have different names and parameters, so a dynamic target alone isn't enough.
- One connection can also control many devices, such as a PTZ controller plugin that drives eight cameras. So a target isn't always a whole connection.
- Bindings have the same need. A label or tally colour has to follow the selected camera.
- [ADR 0004](0004-javascript-expressions-and-scripts.md) finds the state paths an expression reads by parsing it with `oxc`. With a dynamic target, the paths aren't known until the expression runs.

## Decision

### Targets
- **A target names what an action acts on:** a connection, and optionally one item in a collection the connection exposes.

  ```ts
  type Target = { conn: string; key?: PathSegment };   // connection ID (ULID); key per ADR 0017
  ```

- **An action step's target is either fixed or an expression.** A dynamic target declares a **constraint**: a plugin ID or an interface. The editor uses the constraint to type-check the step's parameters, and the runtime checks the resolved target against it.
- **Targets can be stored in state.** The state schema gains `kind: "target"`, with an optional `interface` constraint, so a variable or page parameter can hold "the selected camera". The stored value is the ID form, so renaming an alias never breaks it ([state spec §1.1](../specs/state-and-event-log.md)).
- **In expressions, `target(t)` returns a read-only view** of the target's state, already narrowed to `key` and, when the target came through an interface, mapped to the interface's names: `target(vars.selectedCam).tally.program`. It also exposes `.alias`, `.name` and `.connected`.
- **If a dynamic target resolves to nothing, or to something that doesn't satisfy the constraint, the step fails** with `no_target` and follows its normal error policy. Nothing is dispatched.
- **The journal records the resolved target.** `action.requested` already carries `conn`. It gains `key?` and `via?` (the interface used), so "why did this fire?" shows which camera was actually used.

### Interfaces
- **An interface is a named, versioned contract** for one kind of device, maintained by the Covalence project in `protocol/`: `camera.ptz@1`, `switcher@1`, `router@1`, `mixer.audio@1`, `recorder@1`, and more over time. It declares:
  - actions, with parameter and result schemas
  - events, with payload schemas
  - state, as paths with schemas
- **A plugin declares which interfaces it implements** in its manifest, and maps each member to its own actions and state. Names that match need no mapping. Plugins keep their native action names, and the core translates.

  ```ts
  implements?: {
    [iface: string]: {                       // "camera.ptz@1"
      at?: Path;                             // collection path for per-item interfaces: ["cameras"]
      actions?: { [ifaceAction: string]: string };   // "recallPreset" → "presetRecall"
      state?:   { [ifacePath: string]: Path };       // "tally.program" → ["tally", "pgm"]
      events?:  { [ifaceEvent: string]: string };
    };
  };
  ```

- **`at` handles one connection controlling many devices.** Each item in that collection is a separate target, `{conn, key}`.
- **Conformance is checked twice:** by the SDK at build time, and by the core when the plugin loads. The plugin's parameter schemas must accept the interface's parameters, and its state schemas must match. A plugin that fails the check for an interface doesn't get that interface. Its other features are unaffected.
- **Optional members.** Interfaces may mark members as optional (for example `camera.ptz@1` `focus`). `target(t).supports("focus")` lets bindings and conditions adapt.
- **Plugins can extend an interface** with their own actions, but those are only reachable with a plugin-constrained target, not an interface one.

### Dependency tracking (amends ADR 0004; accepted in ADR 0021)
- **Bindings and state triggers track their dependencies at run time.** Every evaluation records the state paths it actually reads, through the state proxy and through `target()`. The core subscribes to that read set and re-evaluates when any path in it changes. After each evaluation the read set is replaced, so a binding that follows `vars.selectedCam` moves to the new camera's paths as soon as the variable changes.
- **Re-evaluation happens once per scheduler turn,** after patches are coalesced ([state spec](../specs/state-and-event-log.md)). An expression never sees a half-applied turn.
- **`oxc` static extraction stays, but for the editor only:** validation, autocomplete, "where is this used?", and warnings for paths that don't exist. It's no longer the source of truth for reactivity.

## Consequences
- The "control any camera" page needs one page and a selector row, with no duplication. With interfaces it works across camera brands.
- Interfaces become a long-term API surface for the project. Each change to one needs a new version, and plugins need time to adopt it. Start with a few interfaces, based on real plugins, rather than designing them up front.
- Plugin authors have extra work to map their plugin to interfaces. The SDK should make the common case (matching names) zero-config, and the registry can show which interfaces a plugin implements.
- Runtime dependency tracking costs a little on every evaluation, for recording reads. That's small next to evaluating the expression, but it should be measured against the 16 ms re-render budget.
- Static analysis can no longer prove which connections a binding or flow touches. The UI shows the dynamic case as "any `camera.ptz@1`".
- Permissions, the output gate and simulate mode are unchanged. They apply to the resolved target exactly as they would to a fixed one.

## Alternatives considered
- **Fixed targets, as in Companion:** simple and fully static, but forces page duplication, which is the problem this ADR exists to solve.
- **Duck typing** (any connection that has an action called `recallPreset`): no new concept, but names and parameters don't line up across plugins, and nothing checks that they mean the same thing.
- **An "aggregator" plugin** that wraps several camera connections and exposes one: possible today, but each one has to be written by hand, and it hides the real target from the journal.
- **Interfaces defined by plugins, not the project:** more open, but leads to several incompatible "camera" interfaces. A project-maintained set, open to contributions, keeps one meaning per name.
- **Keep static dependency extraction and forbid dynamic reads in bindings:** keeps ADR 0004 unchanged, but makes the selected-camera label impossible, which is half of the use case.
