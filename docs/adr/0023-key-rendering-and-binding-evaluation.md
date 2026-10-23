# 0023 – Keys are rendered in the core; structured bindings are evaluated without QuickJS

- **Status:** Accepted
- **Amended by:** [0026](0026-sources-and-selectors.md) (binding kinds become stateless transforms; native evaluation stands)
- **Date:** 2026-10-22
- **Specs:** [bindings](../specs/bindings.md), [surfaces](../specs/surfaces.md)
- **Related:** [ADR 0021](0021-expressions-as-sandboxed-snippets.md), [ADR 0022](0022-surfaces-pages-and-the-web-surface.md)
- **Milestone:** [M5: Surfaces and bindings](../milestones/m5-surfaces-and-bindings.md)

## Context
- The core renders key images at each control's native size (plugin protocol §8). Nothing in the core draws yet.
- The budget from a state change to a redrawn key is 16 ms ([architecture](../architecture.md#performance-budgets-targets-to-test-against)).
- M4 measured an expression evaluation at ~265 µs in a fresh QuickJS context, ~190 µs of it parsing the prelude. A 32-key page whose bindings all change at once would spend ~8.5 ms evaluating before anything is drawn, and a page has several properties per key.
- M4's two speedups both have a cost. Loading the prelude as bytecode (~95 µs) needs `unsafe`, which the workspace denies. A frozen, reused context needs a lockdown of every built-in to be safe.
- The bindings spec says four of the five binding kinds (`value`, `template`, `rules`, `map`) "compile to an expression". Their reads are known without running them.
- M4 evaluates expressions on whichever async worker needs them, sharing one engine (M4 Phase 2 known gap). A slow expression delays the others.

## Decision
- **Rendering uses `tiny-skia` for drawing and `cosmic-text` for text,** both pure Rust. The core ships one font, Inter (OFL), embedded in the binary. M5 encodes PNG only; JPEG and BMP arrive with hardware surfaces.
- **The default key** is a black background with a white label. A label wraps to at most three lines and shrinks to fit before it's truncated. A key with no binding is blank.
- **A stale key** is drawn at half brightness with a small marker in its top corner (bindings §3).
- **`value`, `template`, `rules` and `map` are evaluated natively in Rust,** reading the state tree directly, with their read sets known when they're saved. They still compile to an expression, which the editor shows and uses for one-way conversion to `expr`. Tests evaluate every binding fixture both ways and require the same result, including `format`s against the standard library (`mmss`, `hhmmss` …).
- **`expr` bindings use QuickJS on a dedicated binding thread,** with its own evaluator, so bindings and flows never wait on each other. Bindings get a 10 ms time limit instead of 50 ms; over that, the binding uses its fallback and reports `exprTimeout`.
- **One binding turn** takes everything waiting on the state feed (as the `state` trigger watcher does), re-evaluates the touched bindings on shown pages, diffs each control's resolved look against the last one drawn, and renders and sends only what changed.

## Consequences
- The common bindings (a label, a tally colour) cost microseconds, so the 16 ms budget is mostly left for `expr` bindings and drawing.
- There are two evaluators for the same semantics. The both-ways fixture tests are what keep them equal; a new `format` or condition `op` must land in both.
- `cosmic-text` brings shaping, wrapping and fallback for non-Latin labels. It's the largest new dependency in the core, which matters on a Raspberry Pi 5; a build without it isn't planned.
- Emoji and icon fonts aren't shipped. A label with an emoji draws Inter's missing-glyph box until an icon story arrives with assets.
- A binding thread is a second QuickJS runtime and its memory. One thread is enough for M5; a pool can follow if a page of `expr` bindings misses the budget.

## Alternatives considered
- **Every kind through QuickJS:** one evaluator and simpler code, but it spends the budget on the bindings that need it least.
- **Bytecode prelude:** halves the cost of every evaluation, `expr` included. It needs an `unsafe` exception in the core for a gain the native kinds mostly make unnecessary. It stays an option for `expr` bindings if they miss the budget.
- **A frozen, reused context:** the fastest option, but a context shared between evaluations is only safe with every built-in locked down, and a missed one lets one binding change what another sees.
- **Rendering in the browser (send the look, not the image):** cheaper for the web surface, but hardware needs images from the core anyway, and two renderers would draw keys differently.
- **`fontdue` or `ab_glyph` instead of `cosmic-text`:** smaller, but no shaping or fallback, so labels in Arabic, Hindi or with combining marks would draw wrongly.
- **Skia or a system graphics library:** better text, but a C++ build in a pure-Rust core and a harder cross-compile to ARM.
