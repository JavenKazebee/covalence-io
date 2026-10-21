import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { defineConfig, devices } from "@playwright/test";

// The M2 browser smoke test (e2e/). It drives the real server, serving the
// built UI, with the real Deno plugin host and the Videohub plugin in
// simulate mode. Run `pnpm --filter ui build` first.

/** Not :7680, so a dev server left running doesn't get in the way. */
const PORT = 7690;
const root = resolve(import.meta.dirname, "../..");
const data = mkdtempSync(join(tmpdir(), "covalence-smoke-"));

export default defineConfig({
  testDir: "e2e",
  testMatch: "*.e2e.ts",
  // The tests share one server and build on each other's connections.
  workers: 1,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `cargo run --locked -p covalence-server -- --listen 127.0.0.1:${PORT} --data ${data} --ui packages/ui/dist`,
    cwd: root,
    url: `http://127.0.0.1:${PORT}/health`,
    // Includes compiling the server on a cold cache.
    timeout: 10 * 60_000,
    reuseExistingServer: false,
    stdout: "pipe",
  },
});
