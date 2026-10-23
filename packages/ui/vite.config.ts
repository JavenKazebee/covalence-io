import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vitest/config";

/** Where `cargo run -p covalence-server` listens by default. */
const server = "http://127.0.0.1:7680";

// The HTTP API, as listed in crates/covalence-server/src/lib.rs.
const api = [
  "/health",
  "/plugins",
  "/connections",
  "/state",
  "/actions",
  "/journal",
  "/flows",
  "/runs",
  "/expressions",
  "/surfaces",
  "/layers",
  "/control-bindings",
];

export default defineConfig({
  plugins: [vue()],
  server: {
    proxy: {
      ...Object.fromEntries(api.map((path) => [path, server])),
      "/ws": { target: server, ws: true },
      // The web surface's socket (surfaces spec §4). `/surfaces` is above.
      "^/surface/": { target: server, ws: true },
    },
  },
  test: {
    environment: "node",
  },
});
