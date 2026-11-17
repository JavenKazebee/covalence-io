import { describe, expect, it } from "vitest";
import type { JsonSchema } from "../api";
import {
  freshStepId,
  fromDraft,
  newFlowDraft,
  isDuration,
  paramProblems,
  problemsAt,
  toDraft,
  type Flow,
  type ParamsSchema,
  type Problem,
  describeTrigger,
  firesOn,
  pressTrigger,
} from "./model";
import { flowStatuses } from "./status";

// The fixtures the Rust tests use too (ADR 0020): valid flows, as stored and served.
const fixtures = import.meta.glob<Flow>("../../../../crates/covalence-core/fixtures/flows/*.json", {
  eager: true,
  import: "default",
});
const problemFixtures = import.meta.glob<{ flow: Flow; errors: Problem[]; warnings: Problem[] }>(
  "../../../../crates/covalence-core/fixtures/flows/problems/*.json",
  { eager: true, import: "default" },
);

// Videohub's `route`, as its manifest has it.
const route: JsonSchema = {
  type: "object",
  required: ["output", "input"],
  properties: { output: { type: "integer", minimum: 0 }, input: { type: "integer", minimum: 0 } },
};
const VH = ["01JBZ7P0D3F5G7H9J1K3L5M7N9", "01JBZ7P0D3F5G7H9J1K3L5M7NA"];
const schemaOf: ParamsSchema = (conn, action) => (VH.includes(conn) && action === "route" ? route : null);

describe("flow model", () => {
  it("has the shared fixtures", () => {
    expect(Object.keys(fixtures).length).toBeGreaterThanOrEqual(2);
    expect(Object.keys(problemFixtures).length).toBeGreaterThanOrEqual(1);
  });

  it.each(Object.entries(fixtures))("round-trips %s through the editor unchanged", (_, flow) => {
    const result = fromDraft(toDraft(flow, schemaOf), schemaOf);
    expect(result).toEqual({ ok: true, flow });
  });

  it("keeps params it has no schema for", () => {
    const { flow } = Object.values(problemFixtures).find((f) => f.flow.name === "Echo")!;
    const result = fromDraft(toDraft(flow, schemaOf), schemaOf);
    expect(result.ok && result.flow.body[1]).toMatchObject({ params: { free: "form" } });
  });

  it("reports bad params by step and field before sending", () => {
    const flow = fixtures["../../../../crates/covalence-core/fixtures/flows/route-twice.json"]!;
    const draft = toDraft(flow, schemaOf);
    draft.body[0]!.values.input = "-1";
    draft.body[2]!.values.output = "";
    const result = fromDraft(draft, schemaOf);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.errors.get(draft.body[0]!.key)).toEqual({ input: "At least 0" });
    expect(result.errors.get(draft.body[2]!.key)).toEqual({ output: "Required" });
  });

  it("places server problems under the field they name", () => {
    const { errors } = Object.values(problemFixtures).find((f) => f.flow.name === " ")!;
    const shown = errors.map((p) => ({ ...p, message: p.code, warning: false }));
    expect(problemsAt(shown, ["body", 1], [["body", 1, "params"]]).map((p) => p.code)).toEqual(["duplicateId"]);
    expect(paramProblems(shown, 1)).toEqual({ input: "badParams", output: "badParams", x: "badParams" });
    expect(problemsAt(shown, ["triggers", 1]).map((p) => p.code)).toEqual(["unknownConnection"]);
  });

  it("keeps expression params apart from literal ones", () => {
    const flow = fixtures["../../../../crates/covalence-core/fixtures/flows/expressions.json"]!;
    const draft = toDraft(flow, schemaOf);
    const branch = draft.body[1]!;
    const high = branch.arms[0]!.steps[0]!;
    expect(high.sources).toEqual({ input: { expr: "local.input" } });
    expect(high.values).toEqual({ output: "1", input: "" });
    expect(branch.otherwise?.[0]?.forSource).toEqual({ expr: "vars.pauseMs ?? 100" });
    // A required param given as an expression isn't missing.
    high.values.output = "";
    const result = fromDraft(draft, schemaOf);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.errors.get(high.key)).toEqual({ output: "Required" });
  });

  it("places problems under nested steps", () => {
    const shown = [{ path: ["body", 1, "arms", 0, "steps", 0, "params", "input"], code: "exprSyntax", message: "bad", warning: false }];
    expect(paramProblems(shown, ["body", 1, "arms", 0, "steps", 0])).toEqual({ input: "bad" });
  });

  it("leaves out an empty filter and no conditions", () => {
    const draft = newFlowDraft();
    draft.name = "f";
    draft.triggers = [{ kind: "event", from: { target: VH[0]! }, event: "routeChanged", filter: " " }];
    const result = fromDraft(draft, schemaOf);
    expect(result.ok && result.flow.triggers[0]).toEqual({ kind: "event", from: { target: VH[0] }, event: "routeChanged" });
    expect(result.ok && "conditions" in result.flow).toBe(false);
  });

  it("reads which controls a trigger fires on, as written", () => {
    const flow = fixtures["../../../../crates/covalence-core/fixtures/flows/events.json"]!;
    const layer = "01JBZ7P0D3F5G7H9J1K3L5M7PA";
    const keys = flow.triggers[4]!;
    expect(firesOn(keys, layer, "k1", "press")).toBe(false); // `any` with an `expr`: not a plain list
    expect(firesOn(pressTrigger(layer, "k0"), layer, "k0", "press")).toBe(true);
    expect(firesOn(pressTrigger(layer, "k0"), layer, "k0", "release")).toBe(false);
    expect(firesOn({ kind: "event", from: { layer, controls: ["k1", "k2"] }, event: ["press", "long"] }, layer, "k2", "long")).toBe(true);
    expect(firesOn({ kind: "event", from: { layer }, event: "*" }, layer, "k9", "turn")).toBe(true);
    expect(describeTrigger(flow.triggers[0]!, (c) => c)).toBe("sys startup");
    expect(describeTrigger(flow.triggers[2]!, (c) => c)).toBe("surface.*.controls.* press");
    expect(describeTrigger(flow.triggers[5]!, (c) => c)).toBe("surface.*.controls.*.pressed (rising)");
  });

  it("keeps a core target, and one held in vars with its constraint", () => {
    const flow: Flow = {
      id: "f", name: "f", slug: "f", enabled: true, triggers: [{ kind: "api" }], concurrency: "ignore", failover: "abort",
      body: [
        { id: "a", kind: "action", target: { conn: "core", key: "S1" }, action: "navigate", params: { layer: "L" } },
        {
          id: "b", kind: "action", target: { ref: { root: "vars", path: ["deck"] } }, constraint: { interface: "surface@1" },
          action: "inject", params: { control: "k1", input: "down", value: { value: { ref: "not a source" } } },
        },
      ],
    };
    const draft = toDraft(flow, () => null);
    expect(draft.body[0]!.targetKey).toBe("S1");
    expect(draft.body[1]!.targetSource).toEqual({ ref: { root: "vars", path: ["deck"] } });
    expect(draft.body[1]!.params.value).toEqual({ ref: "not a source" });
    expect(fromDraft(draft, () => null)).toEqual({ ok: true, flow });
  });

  it("checks durations as the server does", () => {
    for (const ok of ["500ms", "2s", "1m30s", "1h", "1h2m3s4ms"]) expect(isDuration(ok)).toBe(true);
    for (const bad of ["", "0s", "1.5s", "2 s", "s", "1s1m"]) expect(isDuration(bad)).toBe(false);
  });

  it("picks fresh step IDs", () => {
    expect(freshStepId("route", ["route1", "wait1"])).toBe("route2");
    expect(freshStepId("Bad id", [])).toBe("step1");
  });
});

describe("flow status", () => {
  const leaf = (path: string, value: unknown) => ({ path, value, quality: "good" as const, ts: 0, seq: 1 });

  it("reads running and lastRun, as one leaf per field or one object", () => {
    const statuses = flowStatuses([
      leaf("flow.routeTwice.running", 1),
      leaf("flow.routeTwice.lastRun.status", "failed"),
      leaf("flow.routeTwice.lastRun.error.code", "timeout"),
      leaf("flow.followVh.lastRun", { status: "ok", durationMs: 12 }),
    ]);
    expect(statuses.get("routeTwice")).toEqual({ running: 1, lastRun: { status: "failed", error: { code: "timeout" } } });
    expect(statuses.get("followVh")).toEqual({ running: 0, lastRun: { status: "ok", durationMs: 12 } });
  });
});
