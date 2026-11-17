// Text with values in it; `undefined` if any value is.
export default defineTransform({
  id: "template",
  version: 1,
  kind: "stateless",
  options: { parts: { type: "array" } },
  result: { type: "string" },
  compute: ({ options: { parts } }) =>
    parts.some((p) => p === undefined) ? undefined : parts.map((p) => String(p)).join(""),
});
