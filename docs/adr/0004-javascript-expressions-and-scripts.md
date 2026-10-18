# 0004 – JS/TS for expressions and scripts (QuickJS in the core)

- **Status:** Accepted
- **Date:** 2026-10-18

## Context
Expressions appear everywhere: labels, colours, conditions, triggers, action parameters. Users also need script steps for custom logic. Requirements:
- safe and bounded
- fast
- able to list which state values an expression reads, for reactive bindings
- friendly errors
- familiar syntax
- identical behaviour wherever the code runs

## Decision
**One language, JavaScript/TypeScript, with more power at each level:**

| Level | Allowed | Runs in |
|---|---|---|
| Expression | JS **expression syntax only**: no statements, loops, assignments or function definitions | QuickJS in the core |
| Script step | Full JS/TS, `async/await`, time and memory limits | QuickJS in the core |
| Custom plugin | Everything, including npm | Deno plugin host |

- **Engine:** `rquickjs` on QuickJS-ng.
- **Validation and dependencies:** the core parses every expression and script with `oxc`. For expressions it rejects anything that isn't expression syntax and extracts the state paths read. TypeScript is type-stripped before running.
- **Host API in scripts:**
  - `state` (read), `action(id, params)`, `emit(event, payload)`
  - `wait(ms)`, `waitUntil(fn, timeout)`
  - `setVar(name, value)`, `log`
  - `now()`, `random()`
- **No direct network or file access in scripts.** Side effects go through `action()`, so they pass the output gate and are journaled.
- **Missing or offline values** read as `undefined` rather than throwing. Bindings may declare a fallback.
- **Small standard library:** `mmss`, `hhmmss`, `clamp`, `round`, `pad`, `lerp`, `rgb`, `hsl`, `mix`.
- **Editor:** generated TypeScript definitions from the live state tree and plugin manifests.
  - CodeMirror 6 for inline expression fields
  - Monaco, with the TypeScript language service, for script steps
  - Live preview is evaluated by the core over the API

## Consequences
- What users learn in expressions carries straight into scripts.
- One engine means expressions and scripts behave exactly alike.
- Scripts run next to the state and the flow runtime: no IPC hops, cancellation at any instruction, and compatibility with standby shadowing and replay.
- QuickJS is 20–50× slower than V8 on heavy computation. That's irrelevant for glue logic, and heavy work belongs in a plugin.

## Alternatives considered
- **CEL:** safe and analysable, but unfamiliar, with strict number types (`1 == 1.0` is an error). It's the fallback if this approach proves too costly.
- **A custom JS-subset evaluator:** would drift from real JS behaviour.
- **Lua, Rhai, JSONata, spreadsheet formulas, a custom syntax:** a second language for users to learn, or weak browser and Rust support, or a language to maintain forever.
- **Running scripts in the Deno host:** faster, with npm, but adds IPC, shares a crash domain with plugins, and complicates failover.
