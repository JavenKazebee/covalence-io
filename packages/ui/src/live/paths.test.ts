import { describe, expect, it } from "vitest";
import { formatPath, splitPath } from "./paths";

describe("text paths", () => {
  it("formats identifiers, indices and other keys as the server does", () => {
    expect(formatPath("vh", ["outputs", 3, "source"])).toBe("vh.outputs[3].source");
    expect(formatPath("vh", ["outputs", "SDI 4", "source"])).toBe('vh.outputs["SDI 4"].source');
    expect(formatPath("vh", [])).toBe("vh");
  });

  it("splits what it formats", () => {
    for (const segments of [["outputs", 3, "source"], ["outputs", 'say "hi"', "name"], ["model"], []]) {
      const text = formatPath("vh", segments);
      expect(splitPath(text)).toEqual({ root: "vh", segments, end: text.length });
    }
  });

  it("stops at anything it can't read", () => {
    expect(splitPath("vh.outputs[x].source")).toEqual({ root: "vh", segments: ["outputs"], end: 10 });
  });
});
