import { assertEquals } from "@std/assert";
import { s } from "./mod.ts";
import { validate, withDefaults } from "./schema.ts";

const params = s.object({
  output: s.integer({ minimum: 0, choices: { from: "outputs" } }),
  label: s.optional(s.string({ label: "Label" })),
  port: s.integer({ default: 9990 }),
});

Deno.test("builders emit plain JSON Schema", () => {
  assertEquals(params, {
    type: "object",
    properties: {
      output: { type: "integer", minimum: 0, "x-choices": { from: "outputs" } },
      label: { type: "string", title: "Label" },
      port: { type: "integer", default: 9990 },
    },
    required: ["output"],
    additionalProperties: false,
  });
  assertEquals(
    s.collection({ key: "index", label: "name" }, { name: s.string() }),
    {
      collection: { key: "index", label: "name" },
      item: { name: { type: "string" } },
    },
  );
});

Deno.test("validate reports the first problem", () => {
  assertEquals(validate(params, { output: 1 }), undefined);
  assertEquals(validate(params, {}), "output is required");
  assertEquals(validate(params, { output: -1 }), "output must be at least 0");
  assertEquals(validate(params, { output: 1.5 }), "output must be an integer");
  assertEquals(validate(params, { output: 1, x: 1 }), "x is not allowed");
  assertEquals(validate(params, "x", "params"), "params must be an object");
});

Deno.test("withDefaults fills in defaults", () => {
  assertEquals(withDefaults(params, { output: 1 }), { output: 1, port: 9990 });
});
