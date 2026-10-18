/**
 * Deno plugin host: runs plugins for the core, one worker per connection
 * (ADR 0003). Speaks the plugin protocol on stdio; see README.md for how the
 * core launches it.
 *
 * @module
 */

import { formatFromEnv, PROTOCOL_VERSION } from "@covalence-io/protocol";
import { Host, HOST_VERSION } from "./host.ts";

export { Host, HOST_VERSION } from "./host.ts";

if (import.meta.main) {
  // Stdout carries the protocol; keep stray console output off it.
  console.log = console.info = console.debug = console.error;
  const format = formatFromEnv();
  console.error(
    `covalence plugin host ${HOST_VERSION} (protocol ${PROTOCOL_VERSION}, ${format})`,
  );
  const host = new Host({
    readable: Deno.stdin.readable,
    writable: Deno.stdout.writable,
    format,
    onExit: (code) => Deno.exit(code),
  });
  await host.run();
}
