# 0013 – Plugin registry, signing, offline install, pinning

- **Status:** Accepted
- **Date:** 2026-10-18

## Context
Show machines are often offline. A plugin update must never change behaviour mid-show. Plugins come from strangers and run on production systems.

## Decision
- **Package:** a `.cvplugin` archive containing the manifest, the bundled code (all dependencies vendored, nothing fetched at runtime), the licence, simulator files, and a checksum.
- **Signing:** packages are signed with the publisher's key. The registry adds its own signature. The core checks both before installing. The tool (Sigstore or minisign/ed25519) is to be chosen.
- **Registry:** a public index with search, versions, the declared permissions, and badges for "has simulator", CI status and verified publisher.
- **Offline install:** drag a `.cvplugin` file into the UI, or bring plugins along inside a `.covalence` bundle ([0006](0006-project-storage-and-versioning.md)).
- **Pinning:** each project pins exact plugin versions. Updates are proposed, never automatic, and the update review shows the changelog and any **permission changes**.
- **Several versions side by side:** more than one version of a plugin can be installed, so projects pinned to different versions both work.
- **Install shows the plugin's permissions,** for example: "This plugin can connect to {config.host}:{config.port}."

## Consequences
- Needs registry infrastructure and key management. Start with a static, signed index file hosted on GitHub before building a service.
- Vendoring dependencies makes packages larger, but guarantees they work offline and can be reproduced.
