import { describe as group, expect, it } from "vitest";
import type { JournalEntry } from "../live/protocol";
import { describe, describeOrigin, type SummaryContext } from "./summary";

const CONN = "01M5G0BR216VZXJ28P323QT90B";
const CALL = "01M5G0BXZ5X3WHGEVZW8PV0WQS";
const ctx: SummaryContext = {
  alias: (id) => (id === CONN ? "vh" : id),
  callName: (call) => (call === CALL ? "vh.route" : undefined),
};

function entry(kind: string, data: unknown): JournalEntry {
  return { seq: 1, term: 1, ts: "", mono: 0, origin: { conn: CONN }, kind, data };
}

// Shapes as the server journals them.
group("journal summaries", () => {
  it("names state by alias", () => {
    const patch = entry("state.patch", {
      ops: [{ path: { conn: CONN, path: ["outputs", 1, "source"] }, value: 5 }],
    });
    expect(describe(patch, ctx)).toBe("vh.outputs[1].source = 5");
  });

  it("counts the patch ops it doesn't show", () => {
    const ops = [0, 1, 2, 3].map((i) => ({ path: { conn: CONN, path: ["inputs", i, "name"] }, value: `Input ${i}` }));
    expect(describe(entry("state.patch", { ops }), ctx)).toBe(
      'vh.inputs[0].name = "Input 0", vh.inputs[1].name = "Input 1" and 2 more',
    );
  });

  it("follows an action from request to result", () => {
    expect(describe(entry("action.requested", { call: CALL, conn: CONN, action: "route", params: { input: 5, output: 1 } }), ctx)).toBe(
      'vh.route {"input":5,"output":1}',
    );
    expect(describe(entry("action.dispatched", { call: CALL }), ctx)).toBe("vh.route dispatched");
    expect(describe(entry("action.result", { call: CALL, ok: true, durationMs: 2 }), ctx)).toBe("vh.route OK in 2 ms");
    expect(
      describe(entry("action.result", { call: "01XYZABC", ok: false, durationMs: 1, error: { code: "invalid_params", message: "no output 99" } }), ctx),
    ).toBe("call …XYZABC failed in 1 ms: invalid_params: no output 99");
  });

  it("describes connection status and logs", () => {
    expect(describe(entry("conn.status", { conn: CONN, status: "ok" }), ctx)).toBe("vh ok");
    expect(describe(entry("log", { level: "info", message: "plugin host started" }), ctx)).toBe("[info] plugin host started");
  });

  it("describes origins", () => {
    expect(describeOrigin({ user: "api", client: "http" }, ctx)).toBe("api (http)");
    expect(describeOrigin({ conn: CONN }, ctx)).toBe("vh");
    expect(describeOrigin({ core: "system" }, ctx)).toBe("core system");
  });
});
