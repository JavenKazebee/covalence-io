# Spec: Overview

The parts of Covalence, the terms it uses, and the decisions everything else rests on ([Foundations](#foundations)). Each area has its own spec: [state and event log](state-and-event-log.md), [plugin protocol](plugin-protocol.md), [flows](flows.md), [dataflow](dataflow.md), [bindings](bindings.md), [surfaces](surfaces.md), [core connection](core-actions.md) and [composition](composition.md).

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
| **covalence-core** | Rust (tokio) | The engine: state, logs, flows, scheduling, gestures, rendering, device I/O, API, supervision. Runs headless. It has no device drivers: those are plugins ([plugin protocol §13.6](plugin-protocol.md#136-device-drivers-are-plugins-the-core-performs-hardware-io)). |
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
| **Connection** | A configured, running instance of a plugin ("ProPresenter – FOH Mac"). It has an **alias** used in expressions (`pp`). The core itself is a connection, `core` ([core actions §6.1](core-actions.md#61-the-core-is-a-connection)). |
| **Composite** | A connection users build from other connections' state, events and actions, and flows: "Camera A". A composite template is also how devices from different plugins are controlled as one kind ([composition §11.4](composition.md#114-composite-connections)). |
| **Manifest** | What a plugin declares: config, permissions, actions, events, state schema, presets. |
| **Action** | Something a connection can do, with typed parameters (`videohub.route`). |
| **Target** | What an action acts on: a connection, optionally narrowed to one item it controls. Fixed, or chosen at run time ([plugin protocol §13.9](plugin-protocol.md#139-dynamic-targets-proposed)). |
| **Event** | Something that happened at a moment in time, at a node in the state tree (`pp:timerEnded`, `surface.deck.controls.k3:press`) ([dataflow §10.2](dataflow.md#102-events-are-addressed-by-state-path)). |
| **State** | Typed values that persist over time, held in one tree (`pp.timers.countdown.remaining`). |
| **Variable** | User-defined state (`vars.serviceStarted`): written by flows, or computed from a source. |
| **Source** | Where a value comes from: a literal, a state path, an expression or a transform. Every field that can follow state is one ([dataflow §10.1](dataflow.md#101-one-way-to-give-a-value-one-way-to-pick-things)). |
| **Selector** | Which things: controls, targets, paths or entities, by ID, list, region, capability, plugin, template, pattern or expression. |
| **Transform** | A function over sources: stateless (`rules`, `map`, `format`) or a reducer that keeps a value (`toggle`, `position`, `select`). Defined as sandboxed JS, so plugins and projects can add their own ([dataflow §10.3](dataflow.md#103-sources-transforms-and-sinks-helpers-are-transforms)). |
| **Cell** | A source with a home in the state tree: a computed variable, a helper's value, a composite's state. |
| **Sink** | Where a value goes: a control's output, a variable, or an action parameter. |
| **Surface** | A physical or virtual control device: Stream Deck, MIDI controller, fader panel, GPIO box, web panel. Two-way: it takes input and shows output ([surfaces §11.2](surfaces.md#112-surfaces-are-two-way-devices-and-control-input-is-state)). |
| **Control** | One part of a surface, typed by its capabilities: inputs (`momentary`, `relative`, `absolute`, `touch`) and outputs (`image`, `led`, `level`, `text`). |
| **Input state** | What a control reports, kept as state: `pressed`, `detents`, `value`, `touched`. Volatile: reset on attach and detach. |
| **Input event** | An occurrence derived from input state: press, release, double, long, hold, turn, change, touch. |
| **Helper** | A reducer on a control's binding, whose input defaults to that control: a toggle, a position, a velocity, a selection, a script. |
| **Component** | Whatever is on a set of controls, at any size: one key, a strip, a whole page. It holds bindings for its controls and contains other components as **children**, which can repeat over a live list (a key per camera). Parameters make it reusable. A component **shown** on surfaces is a page, a shift layer, a popover or a section: which one controls each control is computed from state (navigation, conditions and rank), never from nesting ([composition §11.8](composition.md#118-one-component-for-keys-strips-and-pages)). |
| **Binding** | Dataflow: a source per property of a control's look (label, colour, image) ([spec](bindings.md)). A binding also holds the control's **helpers**, the input direction. |
| **Template / instance** | The one way to reuse: a composite, component or flow that declares parameters, and entities that use it with their own values. Templates can contain instances of others, and `forEach` makes one instance per item of a live list. Instances never override what's inside ([composition §11.2](composition.md#112-one-template-primitive-for-three-kinds)). |
| **Flow** | The single unit of automation: trigger(s) + optional conditions + a tree of steps. Control flow only. Button action lists, rules and node graphs are all flows ([spec](flows.md)). |
| **Run** | One execution of a flow, with a handle, a status and a cancellation token. |
| **Expression** | A JS expression, used in bindings, conditions and parameters. |
| **Script** | A JS/TS script step inside a flow. |
| **Orbital** | A remote Covalence node that hosts surfaces or plugins on another machine. |

## Composition model

Covalence aims to be flexible without becoming hard to read. A few rules keep it that way:

- **Small primitives that combine.** Sources, selectors, transforms, cells and sinks are each defined once ([dataflow spec](dataflow.md)) and used everywhere. A new selector form or transform works in every place at once.
- **Flows do things in order; dataflow keeps things in step.** Anything with steps, waits or conditions is a flow. Anything that follows state continuously is a source, a cell or a sink.
- **Everything the system can do is an action on a target,** including the core's own (navigate, inject input). Everything that happens is an event at a path.
- **One thing at every size.** A key, a strip and a page are all components, and bigger ones contain smaller ones. Containment recurses; when something is shown doesn't: that's decided once, by the shown component, from state ([composition §11.8](composition.md#118-one-component-for-keys-strips-and-pages)).
- **Reuse is by parameters, never by copying or overriding** ([composition spec](composition.md)). One primitive does it for every kind: a template declares `params`, an instance binds them. Templates nest by containing instances, never by deriving from each other.
- **Scope is explicit.** A container sees its own `params`. To pass a value in, bind a parameter where it's placed, one level at a time.
- **Extensions run where the guarantees are.** Plugins add devices (in the plugin host), and components and transforms (in the core's sandbox). They never add step kinds, scheduling or the rule for which component controls a control.

## Three ways to edit one model

| View | For | Compiles to |
|---|---|---|
| **Button view** | "Press this, do these 3 things." Built live in seconds. | A flow with an event trigger on a control and linear steps |
| **Rule view** | When / if / then cards, schedules, state triggers | A flow with a trigger, a condition and steps |
| **Flow view** | Node graph: branches, loops, parallel paths, waits, script steps | A flow |

All three views show the same structured step tree ([flows §11.1](flows.md#111-flows-are-structured-step-trees-and-control-flow-only)). In the node graph, blocks are nested regions and edges only show order. Values move through expressions, not wires.

You can always move a flow up to a more powerful view ("Open as flow"). A linear flow can also go back down to the simpler view.

## Flow runtime semantics (summary)

The full definition is in the [flow spec](flows.md).

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

## Repo layout

```
covalence-io/
├── protocol/                # protocol schema (single source of truth) → generated types
├── crates/
│   ├── covalence-core/      # engine library, with QuickJS (rquickjs) for expressions and transforms
│   ├── covalence-server/    # headless binary: core + API + UI assets
│   ├── covalence-protocol/  # generated Rust types + framing
│   ├── covalence-store/     # SQLite store, migrations, export/import
│   ├── covalence-devices/   # HID / MIDI / serial I/O and hotplug for plugins (shared with Orbital)
│   └── covalence-orbital/   # Orbital binary (not started)
├── packages/
│   ├── protocol/            # generated TS types
│   ├── sdk/                 # @covalence-io/sdk – plugin SDK (+ simulator toolkit)
│   ├── plugin-host/         # Deno host bootstrap and worker management
│   └── ui/                  # Vue 3 app
├── apps/desktop/            # Tauri shell (not started)
├── plugins/                 # first-party plugins: videohub, streamdeck (later propresenter, red-rcp2, …)
└── docs/
```

Tooling: a Cargo workspace plus a pnpm workspace for the UI and generated types. Plugins are run and tested with Deno's own tooling.

## Foundations

The decisions the whole system rests on, why, and what was rejected. Keep these notes when the sections above change.

### Rust core
- **Decided:** `covalence-core` is Rust on tokio. It owns the state tree, the journal and the project store; the flow runtime, scheduler, gesture layer and output gate; QuickJS; rendering; device I/O for plugins; the HTTP and WebSocket API (`axum`), which serves the UI; and supervision of plugin hosts and Orbitals.
- **Why:** the engine runs shows for 12 hours and more on Windows, macOS, Linux and Raspberry Pi, headless, with low, predictable latency. No garbage-collection pauses, low memory, a small binary that cross-compiles, and show-critical paths (input → trigger → output) that never depend on the plugin runtime. The cost is two languages, and core work moving more slowly than TypeScript would. Plugin authors never need Rust.
- **Rejected:** all TypeScript on Node or Bun (GC and memory growth over long shows, weaker isolation); Go (weaker HID and image-rendering libraries); .NET (a smaller ecosystem in this field, weaker headless and remote-UI story).

### Project entities are Rust types
- **Decided:** project entities are serde types in `covalence-core`, one module per entity, and the source of truth for storage, the HTTP API and validation. Field names follow the specs (camelCase, the names the YAML export uses). The UI mirrors them by hand, and shared fixtures in `crates/covalence-core/fixtures/<entity>/` guard against drift: Rust tests round-trip each one, UI tests type-check and round-trip it, and invalid fixtures carry their expected error codes. TypeSpec stays for the plugin protocol only (plugin protocol §13.2); if the editor needs a machine-readable schema, generate it from the Rust types with `schemars`.
- **Why:** the plugin protocol is a contract with code we don't own; project entities are a contract between the core and our own UI. Validation needs manifests and state, which a schema can't express. Adding a field means a Rust type, a TS type and usually a fixture, and the fixtures make a missed TS edit fail a test rather than a show.
- **Rejected:** TypeSpec for entities (it would only buy the TS side); generating TS from Rust with `ts-rs` or `specta` (a build step before the UI type-checks, and awkward unions in Vue forms; revisit if the mirror drifts despite the fixtures); JSON Schema as the source (`typify` already failed).

### The UI: Vue 3, served by the core, with an optional Tauri shell
- **Decided:** Vue 3, TypeScript, Vite and Pinia, with Vue Flow for the flow view and CodeMirror 6 and Monaco for code. Live state comes over a WebSocket that subscribes to the paths on screen. The core serves the built UI and the API, so any browser works and several can connect at once. Tauri gives an installer, a tray icon, auto-start and a window onto the local UI, and **the engine never runs in the shell**. On first run the core binds to localhost; a setting opens it to the network once a login is configured (state spec §7.5). Main tabs: Connections, Surfaces, Flows, State, Timeline, Settings, with a Cmd+K palette.
- **Why:** quick edits mid-show and complex flow editing, from an iPad in the booth or a laptop at front of house, and headless, with very frequent live updates, which Vue's fine-grained reactivity handles. One codebase covers local, remote and headless use. The browser can't reach HID, which is fine: devices are the core's or an Orbital's.
- **Rejected:** React and xyflow (a stronger node-editor ecosystem, but frequent re-renders need more care); Svelte 5 or Solid (smaller ecosystems); a desktop-only app (no remote or headless use, and the temptation to put the engine in the UI); Electron (heavy).

### Orbital remote nodes (not built)
- **Decided:** Orbital is a small Rust binary for Windows, macOS, Linux and Raspberry Pi, and **a plugin host**: it speaks the plugin protocol, so remote surfaces and plugins appear exactly like local ones. It performs hardware I/O for plugins on its machine, as the core does (plugin protocol §13.6), and can host plugins next to devices on isolated networks. Built-in, permissioned **computer control**: launch and quit apps, send keystrokes, run AppleScript or PowerShell scripts the admin registered (no arbitrary commands by default), off by default and enabled per capability, per Orbital. It finds cores by mDNS, pairs by token or approval in the UI, and uses mutual TLS with a per-node key. It timestamps every input, so gestures are judged by its clock, not network jitter. It reconnects by itself, showing keys as offline meanwhile, and with redundancy connects to both nodes, follows the leader and can be the failover witness.
- **Why:** the engine often runs in a rack room while surfaces are at front of house or on a camera platform, some devices are only reachable from one machine or network, and users want to control software on other computers.
- **Open:** how plugin bundles reach an Orbital, and its discovery port and TXT records (plugin protocol §12).

### Simulated devices
- **Decided:** five kinds of simulation: state-level (`mode: simulate`, the plugin's simulator accepts actions and publishes believable state); protocol-level simulators (fake device servers in the SDK, which double as the plugin's tests); record and replay of host-transport traffic; virtual surfaces in the UI; and **rehearsal mode**, the whole project against simulators with outputs suppressed at the gate, like offline programming on a lighting console.
- **Why:** shows are programmed before the hardware arrives, plugins need tests that don't need a lab, and bugs often show only with a real device's timing. The registry can show a "has simulator" badge. Recordings can hold sensitive data, so sharing one needs a redaction step.

### Licence
- **Decided:** MIT for everything (core, UI, Orbital, SDK, protocol and first-party plugins); third-party plugins choose their own. Contributions use a **Developer Certificate of Origin** (`Signed-off-by:`), not a CLA. "Covalence IO" and the logo are trademarked separately, with a short policy: forks are welcome but must not confuse users with the name. Dependencies must be MIT-compatible, checked in CI (`cargo-deny` and a JS licence check).
- **Why:** fully open and easy to adopt, and vendors and integrators can ship plugins, closed ones included. Anyone can ship closed derivatives; the trademark and the community protect the project, not the licence.
- **Rejected:** Apache-2.0 (adds a patent grant), MPL-2.0 (changed files stay open), GPL-3.0 and AGPL-3.0 (strong copyleft).
