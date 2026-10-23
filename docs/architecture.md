# Architecture overview

## Components

```
                        ┌──────────── Browser / iPad / Tauri shell ───────────┐
                        │          Vue 3 UI  (served by the core)             │
                        └───────────────────────┬─────────────────────────────┘
                                                │ WebSocket + HTTP API
┌──────────────────────────────── covalence-core (Rust) ─────────────────────────────────┐
│                                                                                        │
│  Host Supervisor ──► Input Router ──► Input Events ──► Event Matcher                   │
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
│        ├──► Dataflow Graph (cells, bindings, sinks) ──► Surface Renderer               │
│        │                                                                               │
│  Project Store (SQLite: entities, project op log, snapshots, assets)                   │
│                                                                                        │
│  Device I/O for plugins: HID · MIDI · Serial · hotplug · optional TCP/UDP/HTTP/WS      │
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
| **covalence-core** | Rust (tokio) | The engine: state, logs, flows, scheduling, gestures, rendering, device I/O, API, supervision. Runs headless. It has no device drivers: those are plugins ([ADR 0025](adr/0025-device-drivers-are-plugins.md)). |
| **Plugin host** | TypeScript on Deno | Runs plugins, including surface drivers such as the Stream Deck's. One shared process by default; a connection can be isolated into its own process. |
| **UI** | Vue 3 + TypeScript | The only UI. Served by the core and usable from any browser. |
| **Desktop shell** | Tauri | Optional. Installer, tray icon, auto-start, and a window onto the local UI. Never runs the engine. |
| **Orbital** | Rust | Remote node for surfaces and devices attached to other machines. |

**Rule:** the engine never runs inside a UI process. The show keeps running if every UI window closes or crashes.

## Core concepts

These are the user-facing terms. Product and component names use the chemistry theme; the UI uses plain words.

| Term | Meaning |
|---|---|
| **Plugin** | An installable package that knows how to talk to one kind of device or service, control surfaces included. It can also ship components and transforms. |
| **Connection** | A configured, running instance of a plugin ("ProPresenter – FOH Mac"). It has an **alias** used in expressions (`pp`). The core itself is a connection, `core` ([ADR 0028](adr/0028-the-core-is-a-connection.md)). |
| **Composite** | A connection users build from other connections' state, events and actions, and flows: "Camera A" ([ADR 0030](adr/0030-composite-connections.md)). |
| **Manifest** | What a plugin declares: config, permissions, actions, events, state schema, presets. |
| **Action** | Something a connection can do, with typed parameters (`videohub.route`). |
| **Target** | What an action acts on: a connection, optionally narrowed to one item it controls. Fixed, or chosen at run time ([ADR 0019](adr/0019-dynamic-action-targets-and-interfaces.md)). |
| **Interface** | A versioned contract for one kind of device (`camera.ptz@1`) that plugins implement, so one action or binding works across brands. |
| **Event** | Something that happened at a moment in time, at a node in the state tree (`pp:timerEnded`, `surface.deck.controls.k3:press`) ([ADR 0027](adr/0027-events-addressed-by-path.md)). |
| **State** | Typed values that persist over time, held in one tree (`pp.timers.countdown.remaining`). |
| **Variable** | User-defined state (`vars.serviceStarted`): written by flows, or computed from a source. |
| **Source** | Where a value comes from: a literal, a state path, an expression or a transform. Every field that can follow state is one ([ADR 0026](adr/0026-sources-and-selectors.md)). |
| **Selector** | Which things: controls, targets, paths or entities, by ID, list, region, capability, interface, pattern or expression. |
| **Transform** | A function over sources: stateless (`rules`, `map`, `format`) or a reducer that keeps a value (`toggle`, `position`, `select`). Defined as sandboxed JS, so plugins and projects can add their own ([ADR 0029](adr/0029-dataflow-sources-transforms-sinks.md)). |
| **Cell** | A source with a home in the state tree: a computed variable, a helper's value, a composite's state. |
| **Sink** | Where a value goes: a control's output, a variable, or an action parameter. |
| **Surface** | A physical or virtual control device: Stream Deck, MIDI controller, fader panel, GPIO box, web panel. Two-way: it takes input and shows output ([ADR 0024](adr/0024-surfaces-as-two-way-devices.md)). |
| **Control** | One part of a surface, typed by its capabilities: inputs (`momentary`, `relative`, `absolute`, `touch`) and outputs (`image`, `led`, `level`, `text`). |
| **Input state** | What a control reports, kept as state: `pressed`, `detents`, `value`, `touched`. Volatile: reset on attach and detach. |
| **Input event** | An occurrence derived from input state: press, release, double, long, hold, turn, change, touch. |
| **Helper** | A reducer on a control's binding, whose input defaults to that control: a toggle, a position, a velocity, a selection, a script. |
| **Layer** | A set of control bindings on the controls it covers. Which layer controls each control is computed from state: navigation, conditions and rank. Pages, shift layers, popovers, folders and sections are all layers. |
| **Control group** | Spreads a live list (the selected camera's ISO values) over a set of controls in a layer, with `item` in bindings and triggers. |
| **Binding** | Dataflow: a source per property of a control's look (label, colour, image) ([spec](specs/bindings.md)). A binding also holds the control's **helpers**, the input direction. |
| **Template / instance** | The one way to reuse: a layer, composite, component or flow that declares parameters, and entities that use it with their own values. Templates can contain instances of others, and `forEach` makes one instance per item of a live list. Instances never override what's inside ([ADR 0032](adr/0032-one-template-primitive.md)). |
| **Component** | A template for a reusable control: look, helpers, flows, parameters and other components as parts. Its instances are **placements** on a layer's controls. |
| **Flow** | The single unit of automation: trigger(s) + optional conditions + a tree of steps. Control flow only. Button action lists, rules and node graphs are all flows ([spec](specs/flows.md)). |
| **Run** | One execution of a flow, with a handle, a status and a cancellation token. |
| **Expression** | A JS expression, used in bindings, conditions and parameters. |
| **Script** | A JS/TS script step inside a flow. |
| **Orbital** | A remote Covalence node that hosts surfaces or plugins on another machine. |

## Composition model

Covalence aims to be flexible without becoming hard to read. A few rules keep it that way:

- **Small primitives that combine.** Sources, selectors, transforms, cells and sinks are each defined once ([dataflow spec](specs/dataflow.md)) and used everywhere. A new selector form or transform works in every place at once.
- **Flows do things in order; dataflow keeps things in step.** Anything with steps, waits or conditions is a flow. Anything that follows state continuously is a source, a cell or a sink.
- **Everything the system can do is an action on a target,** including the core's own (navigate, inject input). Everything that happens is an event at a path.
- **Reuse is by parameters, never by copying or overriding** ([composition spec](specs/composition.md)). One primitive does it for every kind: a template declares `params`, an instance binds them. Templates nest by containing instances, never by deriving from each other.
- **Scope is explicit.** A container sees its own `params`. To pass a value in, bind a parameter where it's placed, one level at a time.
- **Extensions run where the guarantees are.** Plugins add devices (in the plugin host), and components and transforms (in the core's sandbox). They never add step kinds, scheduling or layer resolution.

## Three ways to edit one model

| View | For | Compiles to |
|---|---|---|
| **Button view** | "Press this, do these 3 things." Built live in seconds. | A flow with an event trigger on a control and linear steps |
| **Rule view** | When / if / then cards, schedules, state triggers | A flow with a trigger, a condition and steps |
| **Flow view** | Node graph: branches, loops, parallel paths, waits, script steps | A flow |

All three views show the same structured step tree ([ADR 0018](adr/0018-flows-as-step-trees.md)). In the node graph, blocks are nested regions and edges only show order. Values move through expressions, not wires.

You can always move a flow up to a more powerful view ("Open as flow"). A linear flow can also go back down to the simpler view.

## Flow runtime semantics (summary)

The full definition is in the [flow spec](specs/flows.md).

- **Triggers:** an event (a control's, a connection's, a flow's, a helper's or the core's, all addressed by path), a state condition (edge-triggered), a schedule, flow-called-by-flow, and an API or OSC call.
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
│   ├── covalence-devices/   # HID / MIDI / serial I/O and hotplug for plugins (shared with Orbital)
│   └── covalence-orbital/   # Orbital binary
├── packages/
│   ├── protocol/            # generated TS types
│   ├── sdk/                 # @covalence-io/sdk – plugin SDK (+ simulator toolkit)
│   ├── plugin-host/         # Deno host bootstrap and worker management
│   └── ui/                  # Vue 3 app
├── apps/desktop/            # Tauri shell
├── plugins/                 # first-party plugins (videohub, streamdeck, propresenter, red-rcp2, …)
└── docs/
```

Tooling: a Cargo workspace plus a pnpm workspace for the UI and generated types. Plugins are run and tested with Deno's own tooling.
