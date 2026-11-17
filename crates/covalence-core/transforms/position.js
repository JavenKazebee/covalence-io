// A number in [min, max] moved by its input's changes: an encoder's detents.
export default defineTransform({
  id: "position",
  version: 1,
  kind: "reducer",
  options: {
    min: { type: "number", default: 0 },
    max: { type: "number", default: 100 },
    step: { type: "number", default: 1 },
    wrap: { type: "boolean", default: false },
    // Extra movement for fast turns: a change of d moves d * (1 + accel * (|d| - 1)) steps.
    accel: { type: "number", default: 0 },
    initial: { type: "number" },
  },
  steps: { input: "number" },
  result: { type: "number" },
  events: { change: { value: { type: "number" } } },
  init: ({ options }) => (typeof options.initial === "number" ? options.initial : options.min),
  step: (state, { input, previous, options }) => {
    if (typeof input !== "number" || typeof previous !== "number") return undefined;
    const d = input - previous;
    if (d === 0) return undefined;
    const { min, max } = options;
    const moved = d * (1 + options.accel * (Math.abs(d) - 1)) * options.step;
    let next = (typeof state === "number" ? state : min) + moved;
    const span = max - min;
    if (options.wrap && span > 0) {
      next = min + ((((next - min) % span) + span) % span);
    } else {
      next = Math.min(Math.max(next, min), max);
    }
    if (next === state) return undefined;
    return { state: next, value: next, events: [{ name: "change", payload: { value: next } }] };
  },
});
