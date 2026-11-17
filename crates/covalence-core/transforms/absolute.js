// A 0–1 input through a curve. With `pickup`, input is ignored until it
// crosses the followed value.
const shape = (curve, x) =>
  curve === "log" ? Math.log10(1 + 9 * x) : curve === "exp" ? (Math.pow(10, x) - 1) / 9 : x;
const unit = (x) => Math.min(Math.max(x, 0), 1);

export default defineTransform({
  id: "absolute",
  version: 1,
  kind: "reducer",
  options: {
    curve: { type: "string", enum: ["linear", "log", "exp"], default: "linear" },
    pickup: { type: "boolean", default: false },
  },
  steps: { input: "number" },
  result: { type: "number" },
  events: { change: { value: { type: "number" } } },
  init: () => ({ picked: false }),
  initValue: () => undefined,
  step: (state, { input, previous, value, followed, options }) => {
    if (typeof input !== "number") return undefined;
    const y = shape(options.curve, unit(input));
    let picked = state !== null && typeof state === "object" && state.picked === true;
    if (options.pickup && !picked) {
      if (typeof followed !== "number") {
        picked = true;
      } else {
        const p = typeof previous === "number" ? shape(options.curve, unit(previous)) : y;
        picked = Math.abs(y - followed) <= 0.01 || (p - followed) * (y - followed) < 0;
      }
      if (!picked) return { state: { picked: false }, value };
    }
    const out = { state: { picked: true }, value: y };
    if (y !== value) out.events = [{ name: "change", payload: { value: y } }];
    return out;
  },
  // Following a new value means picking it up again.
  follow: (state, { followed }) => ({ state: { picked: false }, value: followed }),
});
