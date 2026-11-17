// Whether any item is truthy; `false` for none.
export default defineTransform({
  id: "any",
  version: 1,
  kind: "stateless",
  options: { of: { type: "array" } },
  result: { type: "boolean" },
  compute: ({ options }) => options.of.some((x) => !!x),
});
