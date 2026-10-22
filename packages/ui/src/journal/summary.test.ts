import { describe as group, expect, it } from "vitest";
import type { JournalEntry } from "../live/protocol";
import { describe, describeOrigin, toneOf, type SummaryContext } from "./summary";

const CONN = "01M5G0BR216VZXJ28P323QT90B";
const CALL = "01M5G0BXZ5X3WHGEVZW8PV0WQS";
const FLOW = "01JC0000000000000000000001";
const RUN = "01JC00000000000000000RUN042";
const ctx: SummaryContext = {
  alias: (id) => (id === CONN ? "vh" : id),
  callName: (call) => (call === CALL ? "vh.route" : undefined),
  flowName: (id) => (id === FLOW ? "Route twice" : id),
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

  it("follows a flow run from trigger to finish", () => {
    expect(describe(entry("trigger.matched", { flow: FLOW, trigger: 0 }), ctx)).toBe("Route twice: trigger 1 matched");
    const blocked = entry("trigger.matched", { flow: FLOW, trigger: 1, blocked: true, reason: "ignore" });
    expect(describe(blocked, ctx)).toBe("Route twice: trigger 2 matched, blocked (ignore)");
    expect(toneOf(blocked)).toBe("warn");
    expect(describe(entry("run.started", { flow: FLOW, run: RUN, mode: "parallel", inputs: {} }), ctx)).toBe(
      "Route twice: run …RUN042 started (parallel)",
    );
    expect(describe(entry("run.step", { run: RUN, step: "route1", status: "started", detail: { position: ["route1"] } }), ctx)).toBe(
      "run …RUN042 step route1 started",
    );
    const failed = entry("run.step", {
      run: RUN,
      step: "wait1",
      status: "failed",
      detail: { position: ["wait1"], error: { code: "cancelled", message: "run cancelled" } },
    });
    expect(describe(failed, ctx)).toBe("run …RUN042 step wait1 failed: cancelled: run cancelled");
    expect(toneOf(failed)).toBe("bad");
    expect(describe(entry("run.finished", { run: RUN, status: "ok" }), ctx)).toBe("run …RUN042 ok");
    expect(toneOf(entry("run.finished", { run: RUN, status: "cancelled" }))).toBe("warn");
  });

  it("describes origins", () => {
    expect(describeOrigin({ flow: FLOW, run: RUN }, ctx)).toBe("flow Route twice");
    expect(describeOrigin({ flow: FLOW, run: RUN, step: "route1" }, ctx)).toBe("flow Route twice › route1");
    expect(describeOrigin({ user: "api", client: "http" }, ctx)).toBe("api (http)");
    expect(describeOrigin({ conn: CONN }, ctx)).toBe("vh");
    expect(describeOrigin({ core: "system" }, ctx)).toBe("core system");
  });
});
