# 0014 – Time sources and scheduling

- **Status:** Proposed
- **Date:** 2026-10-18

## Context
Schedules ("at 9:58 start the countdown"), timers and timecode-driven cues are core production triggers. Redundant nodes must agree on time. Machines sleep, clocks drift, and daylight-saving time changes happen.

## Proposed decision
- **Time-source abstraction in the core from v1.** Sources:
  - system clock (default)
  - system clock with NTP-sync status shown
  - later: LTC (an audio input) and MTC (MIDI), as time-source drivers
  - a monotonic clock for durations and waits
- **Scheduling:**
  - The project has a **timezone**. Schedules use cron-style rules and one-off dates, with explicit daylight-saving behaviour.
  - **Missed-schedule policy per trigger:** `skip` (default) | `run-if-within-grace(duration)` | `run-late`.
  - Schedules are evaluated on the active node only. The standby mirrors the next-fire times.
- **Timecode (v1.x):**
  - Triggers: "at timecode HH:MM:SS:FF", plus ranges, chase and freewheel.
  - State: `sys.timecode.*`.
- **In the state tree:**
  - `sys.time.*`: now, timezone, sync status
  - `sys.timecode.*`

## Open questions
- Is timecode in the first release or v1.x?
- Should an Orbital be able to provide a time source, such as an LTC input at front-of-house?
