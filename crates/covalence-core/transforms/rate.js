// −1 to 1 from the input's distance from a centre, with a deadband: a
// spring-return lever as a speed.
export default defineTransform({
  id: "rate",
  version: 1,
  kind: "reducer",
  options: {
    centre: { type: "number", default: 0.5 },
    deadband: { type: "number", default: 0.05 },
  },
  steps: { input: "number" },
  result: { type: "number" },
  events: {},
  init: () => 0,
  step: (state, { input, options }) => {
    if (typeof input !== "number") return undefined;
    const { centre, deadband } = options;
    const d = input - centre;
    const sign = d < 0 ? -1 : 1;
    const span = d < 0 ? centre : 1 - centre;
    let r;
    if (Math.abs(d) <= deadband) r = 0;
    else if (span - deadband <= 0) r = sign;
    else r = (sign * (Math.abs(d) - deadband)) / (span - deadband);
    r = Math.min(Math.max(r, -1), 1);
    if (r === state) return undefined;
    return { state: r, value: r };
  },
});
