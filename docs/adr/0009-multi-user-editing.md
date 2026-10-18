# 0009 – Multi-user editing, with the server as the single source of truth

- **Status:** Accepted
- **Date:** 2026-10-18

## Context
Several operators (a producer, a technical director, a programmer) may edit or run the same system at once, sometimes during a live show.

## Decision
- **The server is the single source of truth.**
  - Clients send operations against the project op log ([0007](0007-event-sourced-core.md)).
  - The server orders and applies them, then broadcasts them.
  - Each operation carries the entity's **base revision**.
  - Conflicts are resolved field by field: non-overlapping fields merge; overlapping fields are last-write-wins, and the overwritten user is told.
  - No CRDTs.
- **Presence:** who's online, what they're viewing, and a soft lock on the item someone is editing.
- **Undo:** per user, built from inverse operations.
- **Audit trail:** every change is attributed. The timeline shows who changed what, and when.
- **Roles:**
  - `admin` — everything
  - `programmer` — edit the project
  - `operator` — press buttons and run flows, no editing
  - `viewer` — read only
- **Show lock:** puts the project or individual pages into read-only mode for everyone below admin.
- **Draft and publish:** show-critical pages and flows can be edited as a draft and published atomically.

## Consequences
- Needs login: local accounts first, API tokens for automation, SSO later.
- Undo has to cope with other users' changes in between (an inverse operation rebased onto the current state).
