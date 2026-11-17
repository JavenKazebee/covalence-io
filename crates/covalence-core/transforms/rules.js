// The value of the first rule whose `when` is truthy, or `else`.
export default defineTransform({
  id: "rules",
  version: 1,
  kind: "stateless",
  options: { rules: { type: "array" }, else: {} },
  result: {},
  compute: ({ options }) => {
    for (const rule of options.rules) if (rule.when) return rule.value;
    return options.else;
  },
});
