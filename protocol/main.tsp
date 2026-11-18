// Plugin protocol schema: the single source of truth (plugin protocol spec §13.1, §13.2).
// Covers the Milestone 1 subset; see docs/specs/plugin-protocol.md for the full protocol.
import "@typespec/json-schema";
import "./src/envelope.tsp";
import "./src/session.tsp";
import "./src/plugin.tsp";
import "./src/connection.tsp";
import "./src/state.tsp";
import "./src/action.tsp";
import "./src/io.tsp";
import "./src/surface.tsp";
