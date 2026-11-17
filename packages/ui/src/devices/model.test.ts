import { describe, expect, it } from "vitest";
import { asText, clockTime, deviceField, freeAlias, hex16, sizeOf, spacedHex } from "./model";

describe("deviceField", () => {
  it("finds the first property of format device", () => {
    expect(
      deviceField({
        type: "object",
        properties: {
          host: { type: "string", format: "hostname" },
          deck: { type: "string", format: "device" },
        },
      }),
    ).toBe("deck");
    expect(deviceField({})).toBeNull();
  });
});

describe("freeAlias", () => {
  it("takes the plugin ID's last part, numbered if taken", () => {
    expect(freeAlias("io.covalence.streamdeck", [])).toBe("streamdeck");
    expect(freeAlias("io.covalence.streamdeck", ["streamdeck"])).toBe("streamdeck2");
    expect(freeAlias("io.covalence.streamdeck", ["streamdeck", "streamdeck2"])).toBe("streamdeck3");
  });

  it("makes a valid alias of any ID", () => {
    expect(freeAlias("com.acme.my-deck", [])).toBe("my_deck");
    expect(freeAlias("com.acme.9000", [])).toBe("dev_9000");
  });
});

describe("traffic", () => {
  it("spaces hex", () => {
    expect(spacedHex("0a1bff")).toBe("0a 1b ff");
    expect(spacedHex("")).toBe("");
  });

  it("shows line protocols as text and binary as nothing", () => {
    expect(asText("41434b0a")).toBe("ACK\\n");
    expect(asText("0201000000ff")).toBeNull();
    expect(asText("")).toBeNull();
  });

  it("says when a message was cut", () => {
    expect(sizeOf({ ts: 0, dir: "in", hex: "0102", len: 1024 })).toEqual({ len: 1024, cut: true });
    expect(sizeOf({ ts: 0, dir: "in", hex: "0102" })).toEqual({ len: 2, cut: false });
    expect(sizeOf({ ts: 0, dir: "state", note: "opened" })).toBeNull();
  });

  it("formats IDs and times", () => {
    expect(hex16(0xfd9)).toBe("0x0fd9");
    expect(hex16(undefined)).toBe("");
    expect(clockTime(new Date(2026, 0, 1, 9, 5, 3, 42).getTime())).toBe("09:05:03.042");
  });
});
