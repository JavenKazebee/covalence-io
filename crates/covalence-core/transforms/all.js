// Whether every item is truthy; `true` for none.
export default defineTransform({
  id: "all",
  version: 1,
  kind: "stateless",
  options: { of: { type: "array" } },
  result: { type: "boolean" },
  compute: ({ options }) => options.of.every((x) => !!x),
});
