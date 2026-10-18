# 0006 – SQLite working store, text export, migrations

- **Status:** Accepted
- **Date:** 2026-10-18

## Context
Project data must:
- survive a power cut mid-save
- support multi-user editing, undo and replication
- diff well in git
- be shareable as one file
- keep secrets out of shared copies

## Decision
- **Working store: SQLite** in write-ahead-log mode. It holds:
  - materialised entities
  - the project op log ([0007](0007-event-sourced-core.md))
  - snapshots and restore points
  - assets (icons, images)
  - the runtime journal, with limited retention
- **Text export:** a deterministic folder of YAML files, one file per entity, with stable key order and IDs. It can be imported back losslessly, and it's meant for git and code review.
- **Bundle:** a `.covalence` file (zip of the export, plus assets and, optionally, the pinned plugin packages) for sharing.
- **Secrets** (passwords, tokens) live in a separate store: the OS keychain where available, otherwise an encrypted file. Exports and bundles only *reference* secrets.
- **Schema versioning:**
  - The project records its `schemaVersion`, and the app applies ordered migrations.
  - A project from a **newer** version opens read-only, with a warning.
  - **Each plugin versions its own config schema** and ships upgrade functions, which run when a project is opened with a newer version of that plugin.
- **History:**
  - the op log, giving attribution and time-travel (compacted over time)
  - **named restore points**, created manually and automatically before publish, before plugin upgrades and on a schedule
  - **optional built-in git**: commit the text export on publish, and show diffs in the UI

## Consequences
- Crash-safe, atomic saves. The op log feeds undo, multi-user editing and replication from a single mechanism.
- The SQLite file isn't readable in git. The text export covers that.
- Export and import must stay lossless, so they need round-trip tests.

## Alternatives considered
- **Single JSON or YAML file:** noisy diffs, merge conflicts, no atomic partial writes.
- **Folder of files as the source of truth:** good diffs, but no atomic multi-file writes, and a poor fit for multi-user editing.
- **TOML:** awkward for deeply nested data.
