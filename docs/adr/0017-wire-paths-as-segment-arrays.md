# 0017 – State paths on the wire are segment arrays

- **Status:** Accepted
- **Date:** 2026-10-18
- **Resolves:** [plugin protocol](../specs/plugin-protocol.md) open question 2

## Context
- State paths have a text form for people: `vh.outputs["SDI 4"].source` ([state spec §1.1](../specs/state-and-event-log.md)). Collection keys that aren't identifiers need bracket syntax and escaping.
- The store already keeps references in ID form, `{conn, path: [...]}`, so renaming an alias never breaks them.
- State patches are the highest-volume traffic in the system (target: 10,000 updates/s).

## Decision
- **On the wire and in storage, a path is an array of segments:** `["outputs", "SDI 4", "source"]` or `["outputs", 3, "source"]`.

  ```ts
  type PathSegment = string | number;   // number: integer collection key
  type Path = PathSegment[];
  ```

- **The text form is only for people:** expressions, the UI, logs and docs. The core converts between the two forms at those edges.
- Paths in plugin messages stay relative to the connection root, as before.

## Consequences
- No string parsing or escaping on the hot path, and no quoting bugs with keys like `"SDI 4"` or `"a.b"`.
- Wire and storage use the same shape, so no conversion between them.
- MessagePack frames are slightly larger than with dotted strings. The JSON debug mode is a little less compact to read.
- The SDK takes string paths for convenience (`ctx.state.patch("outputs[3].source", 1)`) and converts them before sending.

## Alternatives considered
- **Dotted strings with bracket syntax on the wire:** compact and readable, but every patch has to be parsed and validated, and escaping is easy to get wrong across two languages.
