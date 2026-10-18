# 0011 – MIT licence, DCO, trademarked name

- **Status:** Accepted
- **Date:** 2026-10-18

## Context
The project should be fully open source and easy to adopt. Device vendors and integrators must be able to ship plugins, closed ones included if they choose.

## Decision
- **Licence:** MIT for everything (core, UI, Orbital, SDK, protocol spec and first-party plugins). Third-party plugins choose their own licence.
- **Contributions:** use a **Developer Certificate of Origin** (`Signed-off-by:` on each commit), not a CLA.
- **Trademark:** register "Covalence IO" and the logo separately from the code licence. Forks are welcome, but must not use the name in a way that confuses users. Publish a short trademark policy.

## Consequences
- Maximum adoption and no licensing friction for vendors.
- Anyone, including commercial forks, can ship closed derivatives. The trademark and the community are what protect the project, not the licence.
- Dependencies must be compatible with MIT distribution. Check this in CI (for example `cargo-deny` and a JS licence check).

## Alternatives considered
Apache-2.0 (adds a patent grant), MPL-2.0 (changes to project files must stay open), GPL-3.0 and AGPL-3.0 (strong copyleft).
