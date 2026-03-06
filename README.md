# Covalence IO
**High-performance hardware automation & node-graph engine.**

## Project Structure
- `/crates/engine`: The main Rust logic & event bus.
- `/crates/shared`: Common types used by both backend and extensions.
- `/ui`: Vue 3 + Vite frontend for the node graph and dashboard.

## Development Setup
1. **Backend:** `cargo run -p engine`
2. **Frontend:** `cd ui && npm install && npm run dev`