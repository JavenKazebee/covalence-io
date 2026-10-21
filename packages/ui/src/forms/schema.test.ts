import { describe, expect, it } from "vitest";
import type { JsonSchema } from "../api";
import type { Leaf } from "../live/protocol";
import { collectionChoices, fieldsOf, initialValues, parseValues } from "./schema";

// The Videohub manifest's config schema and `route` params.
const config: JsonSchema = {
  type: "object",
  properties: {
    host: { type: "string", title: "IP address", minLength: 1, format: "hostname" },
    port: { type: "integer", title: "Port", default: 9990, minimum: 1, maximum: 65535 },
  },
  required: ["host"],
};
const route: JsonSchema = {
  type: "object",
  properties: {
    output: { type: "integer", title: "Output", minimum: 0, "x-choices": { from: "outputs" } },
    input: { type: "integer", title: "Input", minimum: 0, "x-choices": { from: "inputs" } },
  },
  required: ["output", "input"],
};

describe("schema forms", () => {
  it("makes a field per property, in order", () => {
    expect(fieldsOf(config).map((f) => [f.key, f.label, f.kind, f.required])).toEqual([
      ["host", "IP address", "text", true],
      ["port", "Port", "integer", false],
    ]);
    expect(fieldsOf(route).map((f) => [f.key, f.kind, f.from])).toEqual([
      ["output", "choices", "outputs"],
      ["input", "choices", "inputs"],
    ]);
  });

  it("puts required fields first, in `required` order", () => {
    const sorted: JsonSchema = { ...route, properties: { input: route.properties!.input!, output: route.properties!.output! } };
    expect(fieldsOf(sorted).map((f) => f.key)).toEqual(["output", "input"]);
    const optionalFirst: JsonSchema = { ...config, properties: { port: config.properties!.port!, host: config.properties!.host! } };
    expect(fieldsOf(optionalFirst).map((f) => f.key)).toEqual(["host", "port"]);
  });

  it("starts from the defaults", () => {
    expect(initialValues(fieldsOf(config))).toEqual({ host: "", port: "9990" });
  });

  it("parses input text into the JSON the schema asks for", () => {
    expect(parseValues(fieldsOf(config), { host: " 10.0.0.5 ", port: "9991" })).toEqual({
      ok: true,
      value: { host: "10.0.0.5", port: 9991 },
    });
    expect(parseValues(fieldsOf(route), { output: "1", input: "5" })).toEqual({
      ok: true,
      value: { output: 1, input: 5 },
    });
  });

  it("leaves out empty optional fields so the server applies defaults", () => {
    expect(parseValues(fieldsOf(config), { host: "h", port: "" })).toEqual({ ok: true, value: { host: "h" } });
  });

  it("reports every bad field", () => {
    expect(parseValues(fieldsOf(config), { host: "", port: "70000" })).toEqual({
      ok: false,
      errors: { host: "Required", port: "At most 65535" },
    });
    expect(parseValues(fieldsOf(config), { host: "h", port: "1.5" })).toEqual({
      ok: false,
      errors: { port: "Not a whole number" },
    });
  });

  it("handles booleans and enums", () => {
    const fields = fieldsOf({
      type: "object",
      properties: { tally: { type: "boolean" }, mode: { type: "integer", enum: [1, 2] } },
    });
    expect(parseValues(fields, { tally: true, mode: "2" })).toEqual({ ok: true, value: { tally: true, mode: 2 } });
    expect(parseValues(fields, { tally: false, mode: "3" })).toEqual({
      ok: false,
      errors: { mode: "Not one of the options" },
    });
  });
});

describe("collection choices", () => {
  const leaf = (path: string, value: unknown): Leaf => ({ path, value, quality: "good", ts: 0, seq: 1 });

  it("lists each item once, labelled", () => {
    const leaves = [
      leaf("vh.inputs[0].name", "Cam 1"),
      leaf("vh.inputs[1].name", "Cam 2"),
      leaf("vh.inputs[1].other", 1),
      leaf('vh.inputs["SDI 4"].other', 2),
    ];
    expect(collectionChoices("vh.inputs", leaves, "name")).toEqual([
      { key: 0, label: "0: Cam 1" },
      { key: 1, label: "1: Cam 2" },
      { key: "SDI 4", label: "SDI 4" },
    ]);
  });
});
