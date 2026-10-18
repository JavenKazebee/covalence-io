// Plugin protocol schema: the single source of truth (ADR 0002, ADR 0016).
// Covers the Milestone 1 subset; see docs/specs/plugin-protocol.md for the full protocol.
import "@typespec/json-schema";
import "./src/envelope.tsp";
import "./src/session.tsp";
import "./src/plugin.tsp";
import "./src/connection.tsp";
import "./src/state.tsp";
import "./src/action.tsp";
