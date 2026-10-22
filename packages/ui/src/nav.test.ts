import { describe, expect, it, vi } from "vitest";

// nav.ts reads `location` and listens for `hashchange` when it loads.
vi.stubGlobal("location", { hash: "" });
vi.stubGlobal("addEventListener", () => {});
const { hrefOf, parseHash } = await import("./nav");

describe("hash routes", () => {
  it("round-trips every route", () => {
    for (const route of [
      { tab: "connections" },
      { tab: "state" },
      { tab: "state", path: 'vh.outputs["SDI 4"]' },
      { tab: "actions" },
      { tab: "flows" },
      { tab: "flows", flow: "01JC0000000000000000000001" },
      { tab: "flows", flow: "new" },
      { tab: "journal" },
      { tab: "journal", seq: 42 },
      { tab: "surfaces" },
      { tab: "surfaces", surface: "01JC0000000000000000000201" },
      { tab: "surfaces", page: "01JBZ7P0D3F5G7H9J1K3L5M7PA" },
      { tab: "surfaces", page: "01JBZ7P0D3F5G7H9J1K3L5M7PA", control: "k3" },
    ] as const) {
      expect(parseHash(hrefOf(route))).toEqual(route);
    }
  });

  it("falls back to Connections, and ignores a bad seq", () => {
    expect(parseHash("")).toEqual({ tab: "connections" });
    expect(parseHash("#/nope")).toEqual({ tab: "connections" });
    expect(parseHash("#/journal/0")).toEqual({ tab: "journal" });
    expect(parseHash("#/journal/x")).toEqual({ tab: "journal" });
  });
});
