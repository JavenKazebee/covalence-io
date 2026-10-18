// Generates TypeScript types for packages/protocol from the bundled JSON Schema
// that TypeSpec emits (ADR 0016).
import { readFileSync, writeFileSync } from "node:fs";
import { compile } from "json-schema-to-typescript";

const [input, output] = process.argv.slice(2);
const bundle = JSON.parse(readFileSync(input, "utf8"));

// TypeSpec emits draft 2020-12:
// - top-level definitions refer to each other by `$id` ("Request.json");
//   rewrite those to `#/$defs/<Name>` and drop the per-def `$id`s
// - anonymous types such as `Record<unknown>` sit in a definition's own nested
//   `$defs` and are referenced as `#/$defs/RecordUnknown`; inline them
// - records use `unevaluatedProperties`, which json-schema-to-typescript
//   ignores; map it to `additionalProperties`
// - a `$ref` with sibling keywords (a property's doc comment) makes
//   json-schema-to-typescript emit a duplicate type; drop the siblings
function rewrite(value, local = {}) {
  if (Array.isArray(value)) return value.map((v) => rewrite(v, local));
  if (value === null || typeof value !== "object") return value;
  if (value.$defs) local = { ...local, ...value.$defs };
  const ref = value.$ref;
  if (typeof ref === "string" && ref.startsWith("#/$defs/")) {
    const name = ref.slice("#/$defs/".length);
    if (name in local) {
      const { $ref: _, ...rest } = value;
      return rewrite({ ...local[name], ...rest }, local);
    }
  }
  if (typeof ref === "string") {
    return { $ref: ref.startsWith("#") ? ref : `#/$defs/${ref.replace(/\.json$/, "")}` };
  }
  const out = {};
  for (let [k, v] of Object.entries(value)) {
    if (k === "$id" || k === "$schema" || k === "$defs") continue;
    if (k === "unevaluatedProperties") k = "additionalProperties";
    out[k] = rewrite(v, local);
  }
  return out;
}

const schema = {
  title: "CovalenceProtocol",
  $defs: rewrite(bundle.$defs),
  // Reference every def from the root so each one is emitted as a named export.
  anyOf: Object.keys(bundle.$defs).map((name) => ({ $ref: `#/$defs/${name}` })),
};

const ts = await compile(schema, "CovalenceProtocol", {
  bannerComment: "// Generated from protocol/*.tsp by protocol/scripts/gen-ts.mjs. Do not edit.",
  additionalProperties: false,
  unreachableDefinitions: true,
  strictIndexSignatures: true,
  format: true,
});
if (output) writeFileSync(output, ts);
else process.stdout.write(ts);
