// A number scaled to a range, or used to pick from a list.
export default defineTransform({
  id: "map",
  version: 1,
  kind: "stateless",
  options: {
    input: { type: "number" },
    from: { type: "array" },
    to: { type: "array" },
    curve: { type: "string", enum: ["linear", "log", "exp"], default: "linear" },
    clamp: { type: "boolean", default: true },
  },
  result: {},
  compute: ({ options: { input: v, from, to, curve, clamp } }) => {
    if (typeof v !== "number") return undefined;
    const [a, b] = from;
    let t = (v - a) / (b - a);
    if (clamp) t = Math.min(Math.max(t, 0), 1);
    if (curve === "log") t = Math.log10(1 + 9 * t);
    else if (curve === "exp") t = (Math.pow(10, t) - 1) / 9;
    const ranged = to.length === 2 && to.every((x) => typeof x === "number");
    if (ranged) {
      const [c, d] = to;
      const out = c + (d - c) * t;
      return Number.isFinite(out) ? out : undefined;
    }
    return to[Math.min(Math.max(Math.floor(t * to.length), 0), to.length - 1)];
  },
});
