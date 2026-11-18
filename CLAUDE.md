# Covalence IO

Open-source live-production control: a Rust core (`crates/`), a Deno plugin host and TypeScript SDK (`packages/`), first-party plugins (`plugins/`), a Vue 3 UI (`packages/ui`) and the plugin protocol schema (`protocol/`). Start with [docs/specs/overview.md](docs/specs/overview.md).

## Docs

There are two kinds of doc, and no others. Don't add architecture decision records, design notes or new top-level docs.

- **Specs** (`docs/specs/`) describe how the system works **now**. When you change behaviour, update the spec in the same commit as the code.
  - Each spec ends with a **Decisions** section: what was decided, why, and the alternatives rejected. When you make a design decision, add or revise an entry there. When you rewrite a section, keep its Decisions entry: it's what stops a rejected alternative coming back.
  - A design that isn't built yet is marked as proposed in its heading or with a milestone marker such as **(M9)**. Revise it in place; don't write a second copy that amends it.
  - Cross-cutting decisions (language, UI stack, Orbital, licence) are in the overview's Foundations.
- **Milestones** (`docs/milestones/`) are plans and checklists. Tick items as they land, with a sentence on what was built if it isn't obvious. Implementation detail belongs in commit messages, not the milestone.
  - When a milestone closes, trim it to its status, goal and "Done when" list (acceptance tests cite those), plus anything still open: known gaps, measurements, checks waiting on hardware. Move anything a spec needs into the spec first.

## Code comments

- To explain why code is the way it is, cite the spec section: `(dataflow spec §5)`, `(plugin protocol spec §13.6)`.
- Don't tag code with milestones or phases ("M8 Phase 2"). That's history, and git has it.

## Checks

- Rust: `cargo fmt --check`, `cargo test --workspace --locked` (needs `deno` on the PATH for the plugin host tests).
- TypeScript: `deno task test`; UI: `pnpm --filter ui type-check lint test`.
- Protocol changes: see `protocol/README.md`.
