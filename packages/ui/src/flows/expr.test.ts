import { describe, expect, it } from "vitest";
import { stepRefs, stepsBefore, stepWarnings } from "./expr";
import { newArmDraft, newStepDraft } from "./model";

describe("step references", () => {
  it("finds steps.<id> in code, not in strings or comments", () => {
    const source = `steps.route1.result.ok && steps?.pick.status // steps.nope
      "steps.alsoNot"`;
    expect(stepRefs(source).map((r) => r.id)).toEqual(["route1", "pick"]);
    expect(stepRefs("const s = steps.a;\nreturn s").map((r) => r.id)).toEqual(["a"]);
    expect(stepRefs("other.steps.a + steps['b']")).toEqual([]);
  });

  it("knows which steps finish first", () => {
    const first = newStepDraft("wait", "first");
    const inner = newStepDraft("wait", "inner");
    const late = newStepDraft("set", "late");
    const pick = newStepDraft("branch", "pick");
    pick.arms = [{ ...newArmDraft(), steps: [inner, late] }];
    const after = newStepDraft("set", "after");
    const body = [first, pick, after];

    const sorted = (set: Set<string>) => [...set].sort();
    expect(sorted(stepsBefore(body, late))).toEqual(["first", "inner"]);
    expect(sorted(stepsBefore(body, after))).toEqual(["first", "inner", "late", "pick"]);
    expect(stepsBefore(body, null).size).toBe(0);

    const all = new Set(["first", "pick", "inner", "late", "after"]);
    expect(stepWarnings("steps.after.status + steps.ghost + steps.first", stepsBefore(body, late), all)).toEqual([
      "Step after hasn't finished when this runs, so steps.after is undefined.",
      "There's no step ghost, so steps.ghost is undefined.",
    ]);
  });
});
