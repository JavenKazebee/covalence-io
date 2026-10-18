/**
 * Deno plugin host: runs plugins for the core, one worker per connection (ADR 0003).
 *
 * @module
 */

import { PROTOCOL_VERSION } from "@covalence-io/protocol";

if (import.meta.main) {
  console.error(`covalence plugin host (protocol ${PROTOCOL_VERSION})`);
}
