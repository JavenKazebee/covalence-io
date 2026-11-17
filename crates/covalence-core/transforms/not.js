export default defineTransform({
  id: "not",
  version: 1,
  kind: "stateless",
  options: { input: {} },
  result: { type: "boolean" },
  compute: ({ options }) => !options.input,
});
