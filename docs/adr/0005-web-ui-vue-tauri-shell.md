# 0005 – Vue 3 web UI served by the core, with an optional Tauri shell

- **Status:** Accepted
- **Date:** 2026-10-18

## Context
The UI must support quick button edits during a live show and also complex flow editing. It must work remotely (an iPad in the booth, a laptop at front-of-house) and headless. It has to handle very frequent live value updates.

## Decision
- **Framework:**
  - Vue 3 + TypeScript + Vite, with Pinia for stores
  - Vue Flow for the node editor
  - CodeMirror 6 and Monaco for code (see [0004](0004-javascript-expressions-and-scripts.md))
- **Live state:** Vue's fine-grained reactivity, fed by a WebSocket subscription. The client subscribes to the state paths currently on screen.
- **Delivery:** the core serves the built UI and API. Any browser works, and several clients can connect at once.
- **Desktop shell:** Tauri provides the installer, tray icon (start, stop, status), auto-start at login, and a window onto the local UI. **The engine never runs in the shell.**
- **Network exposure:** on first run, bind to localhost. A setting opens it to the network once a login is configured.
- **Main tabs:** Connections · Surfaces · Flows · State · Timeline · Settings. A Cmd+K command palette works everywhere.

## Consequences
- One UI codebase covers local, remote and headless use.
- Tauri uses each OS's own web engine (WebKit, WebKitGTK, WebView2). The UI must work in normal browsers anyway, so the target is the same.
- The browser can't reach HID devices directly. That's fine, because surfaces are handled by the core or an Orbital.
- An API reachable over the network needs proper login and roles (see [0009](0009-multi-user-editing.md)).

## Alternatives considered
- **React + xyflow:** the stronger node-editor ecosystem, but re-rendering on frequent live updates needs more care.
- **Svelte 5 / Solid:** fine-grained updates too, but smaller ecosystems.
- **A desktop-only app:** no remote or headless use, and the temptation to put the engine in the UI process.
- **Electron:** consistent rendering, but heavy.
