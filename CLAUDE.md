# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Run the engine
cargo run -p engine

# Build
cargo build

# Check for errors without building
cargo check

# Run tests
cargo test

# Run tests for a specific crate
cargo test -p engine
cargo test -p shared

# Frontend (ui/ directory)
cd ui && npm install && npm run dev
```

## Architecture

This is a **hardware automation and node-graph engine** written in Rust. The workspace has two crates:

- `crates/shared`: Common types (`Value`, `DataType`, `Message`, `Event`) shared across all components
- `crates/engine`: The main binary — event bus, driver layer, and node graph execution

### Event Bus

The backbone of the engine is a `tokio::sync::broadcast::channel<Message>` defined in `main.rs`. Every component communicates via this channel. `Message` carries a `source` string and an `Event` payload, which is either a `Signal` (data from a driver) or a `Command` (targeted action with params).

### Driver Layer (`crates/engine/src/drivers/`)

Drivers are hardware/input abstractions. Each implements the `Driver` trait:
- `id()` — unique driver name
- `start(context, rx)` — async loop that reads from the broadcast channel and updates a shared telemetry cache

`DriverManager` registers drivers, creates per-driver `DriverContext` (wraps the shared `DashMap<String, Value>` cache with a namespaced key prefix), and spawns each driver as a Tokio task. The cache key format is `"{driver_id}/{signal_id}"`.

The only current driver is `VirtualKbDriver` — a stdin-based keyboard simulator for testing.

### Node Graph Engine (`crates/engine/src/nodes/`)

A `petgraph::StableGraph` where nodes are `NodeInstance` and edges are `Edge` (carrying `from_pin`/`to_pin` names).

- **`Node` trait** — implement `id()`, `inputs()`, `outputs()`, and `execute()`. Inputs/outputs return `Vec<Pin>` which can be dynamic based on the instance's `config`.
- **`NodeRegistry`** — maps node type IDs (e.g. `"logic/type_converter"`) to `Box<dyn Node>`.
- **`NodeManager`** — owns the graph and registry. `run_from(uuid)` does BFS to find downstream nodes then executes them in topological order. `run_all()` executes the entire graph in topological order. Execution resolves inputs by reading `last_outputs` from upstream nodes via incoming edges.

### Value System (`crates/shared/src/lib.rs`)

`Value` is the universal data type: `Null | Bool | Float | String | List | Object | Trigger`. `TryFrom<&Value>` conversions are implemented for all concrete Rust types. Nodes receive and return `HashMap<String, Value>` for their input/output pins.

### Adding a New Node

1. Create a struct implementing `Node` in `crates/engine/src/nodes/impls/`
2. Register it in `NodeRegistry` via `registry.register(MyNode)`
3. Node type IDs use slash-namespacing: `"category/node_name"`
