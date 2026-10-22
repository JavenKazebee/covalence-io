# 0021 – Expressions are sandboxed snippets; what code can do depends on where it runs

- **Status:** Accepted
- **Date:** 2026-10-21
- **Amends:** [0004](0004-javascript-expressions-and-scripts.md) (the expression level and the core's use of `oxc`; the language, engine and script host API stand). Takes over the dependency-tracking section of [0019](0019-dynamic-action-targets-and-interfaces.md), which stays proposed for dynamic targets and interfaces.
- **Specs:** [flows](../specs/flows.md), [bindings](../specs/bindings.md)
- **Milestone:** [M4: Expressions](../milestones/m4-expressions.md)

## Context
- [ADR 0004](0004-javascript-expressions-and-scripts.md) has three levels: expressions (JS expression syntax only), script steps (full JS/TS) and plugins. The core parses everything with `oxc`, to reject non-expression syntax and to extract the state paths an expression reads.
- The syntax restriction protected two things: **no side effects**, and **static dependency extraction**. [ADR 0019](0019-dynamic-action-targets-and-interfaces.md) already showed that static extraction can't follow dynamic reads (`vh.outputs[var.n].source`) and proposed tracking reads at run time instead.
- Syntax is a poor proxy for side effects. A lookup table with `const`, a `for` loop over an array or a small helper function changes nothing outside itself, but the rule forbids them. Meanwhile, what could actually change the world is whatever the host hands in, and that's ours to control.
- M4 is the first milestone with QuickJS in the core. JavaScript stays the language: Rhai, Rune, Lua/Luau, CEL and Starlark were compared again, and none outweighs one language shared with TS plugins and a browser editor with generated types.

## Decision
**One language and one engine, and no syntax difference between levels. Code that computes is an expression and may run anywhere, any number of times. Code that acts is a script step and runs once, inside a run.**

| | Expression | Script step |
|---|---|---|
| Used in | Action params, `wait.for`, trigger filters, conditions, `state` `when`, `waitUntil`, `branch`, `set` values, bindings, editor preview | One `script` step in a flow |
| Runs | Whenever the core decides, as often as it needs: on state changes, on the standby node, in live preview | Once, at a known point in a run |
| Syntax | A single expression, or a function body that `return`s | Full JS/TS, `async/await` |
| Can read | `trigger`, `input`, `local`, `steps`, state roots | The same |
| Can act | No | The host API from ADR 0004: `action()`, `setVar()`, `setLocal()`, `emit()`, `wait()`, `waitUntil()`, `log` |
| Time and randomness | None. Time is read from `sys.time.now` | `now()` and `random()`, journaled as `nondet` |

**Expressions are pure by sandbox, not by syntax.**
- **Any syntax.** The core compiles the source as an expression first, then as a function body. `vh.outputs[1].source == 3` and a multi-line body ending in `return` are both expressions.
- **Read-only scope.** Every name in scope is a read-only proxy. A write such as `var.x = 1` throws "state is read-only here; use a `set` step".
- **No host API.** Nothing that acts, waits or emits is in scope.
- **No memory between evaluations.** Each evaluation gets a fresh context, or one whose global object is frozen, so nothing carries over. Phase 1 of M4 measures which is cheaper.
- **No nondeterminism.** `Date.now()`, `new Date()` without arguments and `Math.random()` throw. This keeps replay and standby shadowing exact. It also keeps reactivity honest: `sys.time.now` is state, so an expression that reads it re-evaluates as time passes, and one that read the system clock would not.
- **No async.** No `await`, and a returned promise is an error.

**Everything that runs in the core is bounded and has no way out.**
- **No file, network or process access** for expressions or scripts. Side effects go through `action()`, so they pass the single output gate ([ADR 0007](0007-event-sourced-core.md)), are journaled, are suppressed on a standby node ([ADR 0008](0008-redundancy-and-failover.md)) and don't happen during replay. This also means editing a flow never grants access to the show machine itself.
- **Limits per evaluation:** 50 ms of run time, enforced through the QuickJS interrupt hook, and 16 MB of memory. The defaults are core settings. Scripts get their own, longer time limits (the step's `timeout`); the memory limit applies to both.
- **Failures:** an expression that throws, runs out of time or runs out of memory fails the step it belongs to (codes `exprError`, `exprTimeout`, `exprMemory`, with the message and source position). In a trigger, filter or condition, the failure counts as false and is logged, rate-limited.

**Dependencies are tracked at run time** (from ADR 0019).
- Every evaluation records the state paths it actually reads, through the state proxy. `state` triggers, `waitUntil` and bindings subscribe to that read set and re-evaluate when any path in it changes. The read set is replaced after each evaluation.
- Re-evaluation happens once per scheduler turn, after patches are coalesced ([state spec](../specs/state-and-event-log.md)), so an expression never sees a half-applied turn.

**`oxc` leaves the core for now.**
- QuickJS's compiler reports syntax errors with line and column. The sandbox, not the parser, guarantees purity. Dependencies come from read sets.
- References to step IDs that don't exist (`steps.route9`) are warnings in the editor, which already parses JavaScript for highlighting. At run time they read as `undefined`, like any missing value.
- `oxc` comes back with script steps, for TypeScript type-stripping.

## Consequences
- Users write ordinary JavaScript wherever there's a code field. The one rule to learn is visible from where the code sits: steps act, everything else computes.
- Lookup tables, small loops and helpers work in expressions, which removes the main reason to reach for a script.
- The core no longer proves at save time that a step reference exists. That's a warning in the editor, and the API accepts the flow.
- A fresh or frozen context costs something on every evaluation. That's negligible for flows but has to be measured for surface bindings against the 16 ms re-render budget.
- Read-only proxies and read tracking sit on the hot path of every evaluation. They need a benchmark in M4.
- ADR 0004's three-level table now reads as two contexts sharing one language, plus plugins. Its host API, standard library and editor plans stand.

## Alternatives considered
- **Keep expression-only syntax, checked by `oxc` in the core:** a stronger static guarantee and save-time step checks, but it forbids harmless code, adds a large dependency to the core, and enforces purity in the wrong place.
- **Check syntax with QuickJS plus a hand-written scan for `=`, `++` and `=>`:** no new dependency, but fragile, with shifted error positions.
- **Check only in the editor:** flows saved over the API or imported from files could contain side effects.
- **Give expressions the full host API:** one kind of code, but an expression's evaluation count is an implementation detail. A repaint or a keystroke in the editor's preview could fire an action, and the action would have no run to belong to.
- **No sandbox for scripts, since project editors are trusted:** simpler, but direct I/O bypasses the output gate, so a standby node would send commands twice and replay would touch real devices. A `while (true)` would freeze the core mid-show.
- **Lua/Luau, Rhai, Rune, CEL, Starlark instead of JavaScript:** see the comparison in Context. Luau was the strongest, for speed, sandboxing and familiarity from lighting consoles. It lost on being a second language next to TS plugins, on how it fits the JSON-shaped state tree (1-based arrays, `nil` holes in tables), and on browser editor tooling.
