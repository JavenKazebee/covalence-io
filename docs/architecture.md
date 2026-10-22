# Architecture overview

## Components

```
                        ┌──────────── Browser / iPad / Tauri shell ───────────┐
                        │          Vue 3 UI  (served by the core)             │
                        └───────────────────────┬─────────────────────────────┘
                                                │ WebSocket + HTTP API
┌──────────────────────────────── covalence-core (Rust) ─────────────────────────────────┐
│                                                                                        │
│  Host Supervisor ──► Input Router ──► Gesture Layer ──► Trigger Matcher                │
│   (Deno host,            ▲                                   │                         │
│    isolated hosts,       │                                   ▼                         │
│    Orbitals)             │                             Flow Runtime ──► Script Engine  │
│        ▲  │              │                              (runs, waits,     (QuickJS)    │
│        │  │              │                               cancellation)                 │
│        │  ▼              │                                   │                         │
│     Actions ◄──── Output Gate ◄──────────────────────────────┘                         │
│                  (role, show lock, simulate)                                           │
│                                                                                        │
│  State Tree (reactive) ◄──── Runtime Journal ────► Timeline / Replication              │
│        │                                                                               │
│        ├──► Binding Evaluator ──► Surface Renderer (diffed images)                     │
│        │                                                                               │
│  Project Store (SQLite: entities, project op log, snapshots, assets)                   │
│                                                                                        │
│  Built-in drivers: Stream Deck HID · MIDI · OSC · TCP/UDP/HTTP/WS · Serial             │
└────────────────────────────────────────────────────────────────────────────────────────┘
        ▲ plugin protocol (stdio)                      ▲ plugin protocol (TLS)
┌───────┴────────────┐                         ┌───────┴──────────────────────┐
│ Deno plugin host   │                         │ Orbital (Rust)               │
│  worker per conn   │                         │  remote surfaces, local I/O, │
│  (+ isolated procs)│                         │  remote plugin host          │
└────────────────────┘                         └──────────────────────────────┘
```

| Component | Language | Role |
|---|---|---|
| **covalence-core** | Rust (tokio) | The engine: state, logs, flows, scheduling, gestures, rendering, built-in drivers, API, supervision. Runs headless. |
| **Plugin host** | TypeScript on Deno | Runs plugins. One shared process by default; a connection can be isolated into its own process. |
| **UI** | Vue 3 + TypeScript | The only UI. Served by the core and usable from any browser. |
| **Desktop shell** | Tauri | Optional. Installer, tray icon, auto-start, and a window onto the local UI. Never runs the engine. |
| **Orbital** | Rust | Remote node for surfaces and devices attached to other machines. |

**Rule:** the engine never runs inside a UI process. The show keeps running if every UI window closes or crashes.

## Core concepts

These are the user-facing terms. Product and component names use the chemistry theme; the UI uses plain words.

| Term | Meaning |
|---|---|
| **Plugin** | An installable package that knows how to talk to one kind of device or service. |
| **Connection** | A configured, running instance of a plugin ("ProPresenter – FOH Mac"). It has an **alias** used in expressions (`pp`). |
| **Manifest** | What a plugin declares: config, permissions, actions, events, state schema, presets. |
| **Action** | Something a connection can do, with typed parameters (`videohub.route`). |
| **Target** | What an action acts on: a connection, optionally narrowed to one item it controls. Fixed, or chosen at run time ([ADR 0019](adr/0019-dynamic-action-targets-and-interfaces.md)). |
| **Interface** | A versioned contract for one kind of device (`camera.ptz@1`) that plugins implement, so one action or binding works across brands. |
| **Event** | Something that happened at a moment in time (`pp.timerEnded`). |
| **State** | Typed values that persist over time, held in one tree (`pp.timers.countdown.remaining`). |
| **Variable** | User-defined state (`vars.serviceStarted`). |
| **Surface** | A physical or virtual control device: Stream Deck, MIDI controller, web panel. |
| **Control** | One input or output on a surface: button, encoder, fader, touch strip, display. |
| **Gesture** | An interpreted input: press, release, double-press, long-press, hold-repeat, turn. |
| **Page** | A layout of control bindings shown on a surface. |
| **Binding** | Dataflow: a reactive link from state to a control's look (label, colour, image), stored as a value, template, rules, map or expression ([spec](specs/bindings.md)). Faders and encoders use **value mappings** in the other direction. |
| **Flow** | The single unit of automation: trigger(s) + optional conditions + a tree of steps. Control flow only. Button action lists, rules and node graphs are all flows ([spec](specs/flows.md)). |
| **Run** | One execution of a flow, with a handle, a status and a cancellation token. |
| **Expression** | A JS expression, used in bindings, conditions and parameters. |
| **Script** | A JS/TS script step inside a flow. |
| **Orbital** | A remote Covalence node that hosts surfaces or plugins on another machine. |

## Three ways to edit one model

| View | For | Compiles to |
|---|---|---|
| **Button view** | "Press this, do these 3 things." Built live in seconds. | A flow with a gesture trigger and linear steps |
| **Rule view** | When / if / then cards, schedules, state triggers | A flow with a trigger, a condition and steps |
| **Flow view** | Node graph: branches, loops, parallel paths, waits, script steps | A flow |

All three views show the same structured step tree ([ADR 0018](adr/0018-flows-as-step-trees.md)). In the node graph, blocks are nested regions and edges only show order. Values move through expressions, not wires.

You can always move a flow up to a more powerful view ("Open as flow"). A linear flow can also go back down to the simpler view.

## Flow runtime semantics (summary)

The full definition is in the [flow spec](specs/flows.md).

- **Triggers:** gesture on a control, event, state condition (edge-triggered), schedule, flow-called-by-flow, API or OSC call, and startup.
- **Concurrency mode per flow:** `parallel` | `queue` | `restart` | `ignore` (when a run is already active).
- **Steps:** action, wait (duration), wait-until (expression, timeout), call flow, set variable, script, log, stop, break, continue.
- **Blocks:** group (with catch and finally), branch, loop, parallel.
- **Cancellation:** cooperative at every step boundary and inside scripts, via the QuickJS interrupt hook.
- **Every step is journaled** with causation, so the timeline can answer "why did this fire?"

## Performance budgets (targets to test against)

| Path | Budget |
|---|---|
| Surface key down → first action dispatched | < 5 ms (local surface) |
| Key image re-render after a state change | < 16 ms |
| Sustained state updates without UI stutter | 10,000 / s |
| Connections on a Raspberry Pi 5 | 50+ |
| Plugin host restart → connections restored | < 3 s |

## Proposed repo layout

```
covalence-io/
├── protocol/                # protocol schema (single source of truth) → generated types
├── crates/
│   ├── covalence-core/      # engine library
│   ├── covalence-server/    # headless binary: core + API + UI assets
│   ├── covalence-protocol/  # generated Rust types + framing
│   ├── covalence-store/     # SQLite store, migrations, export/import
│   ├── covalence-script/    # QuickJS embedding, expression validator (oxc)
│   ├── covalence-surfaces/  # Stream Deck / MIDI / X-keys drivers (shared with Orbital)
│   └── covalence-orbital/   # Orbital binary
├── packages/
│   ├── protocol/            # generated TS types
│   ├── sdk/                 # @covalence-io/sdk – plugin SDK (+ simulator toolkit)
│   ├── plugin-host/         # Deno host bootstrap and worker management
│   └── ui/                  # Vue 3 app
├── apps/desktop/            # Tauri shell
├── plugins/                 # first-party plugins (videohub, propresenter, red-rcp2, …)
└── docs/
```

Tooling: a Cargo workspace plus a pnpm workspace for the UI and generated types. Plugins are run and tested with Deno's own tooling.
