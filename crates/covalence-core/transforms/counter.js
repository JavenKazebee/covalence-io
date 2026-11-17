// A number stepped on each event, within optional bounds.
export default defineTransform({
  id: "counter",
  version: 1,
  kind: "reducer",
  options: {
    initial: { type: "number", default: 0 },
    step: { type: "number", default: 1 },
    min: { type: "number" },
    max: { type: "number" },
    wrap: { type: "boolean", default: false },
  },
  steps: { on: ["press"] },
  result: { type: "number" },
  events: { change: { value: { type: "number" } } },
  init: ({ options }) => options.initial,
  step: (state, { event, options }) => {
    if (event === undefined) return undefined;
    const { min, max, wrap } = options;
    const from = typeof state === "number" ? state : options.initial;
    let next = from + options.step;
    if (typeof max === "number" && next > max) next = wrap && typeof min === "number" ? min : max;
    if (typeof min === "number" && next < min) next = wrap && typeof max === "number" ? max : min;
    if (next === state) return undefined;
    return { state: next, value: next, events: [{ name: "change", payload: { value: next } }] };
  },
});
