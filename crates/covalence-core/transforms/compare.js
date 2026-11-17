// A comparison, as a boolean. `==` and `!=` are strict.
export default defineTransform({
  id: "compare",
  version: 1,
  kind: "stateless",
  options: {
    input: {},
    op: { type: "string", enum: ["==", "!=", ">", ">=", "<", "<=", "truthy", "falsy"] },
    value: {},
  },
  result: { type: "boolean" },
  compute: ({ options: { input, op, value } }) => {
    switch (op) {
      case "==": return input === value;
      case "!=": return input !== value;
      case ">": return input > value;
      case ">=": return input >= value;
      case "<": return input < value;
      case "<=": return input <= value;
      case "truthy": return !!input;
      default: return !input;
    }
  },
});
