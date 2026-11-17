// A boolean set by one event and reset by another.
const names = (v) => (Array.isArray(v) ? v : typeof v === "string" ? [v] : []);

export default defineTransform({
  id: "latch",
  version: 1,
  kind: "reducer",
  options: {
    set: { type: ["string", "array"], default: "press" },
    reset: { type: ["string", "array"], default: "release" },
    initial: { type: "boolean", default: false },
  },
  steps: { on: ["*"] },
  result: { type: "boolean" },
  events: { on: {}, off: {} },
  init: ({ options }) => options.initial === true,
  step: (state, { event, options }) => {
    if (event === undefined) return undefined;
    let next;
    if (names(options.set).includes(event.name)) next = true;
    else if (names(options.reset).includes(event.name)) next = false;
    else return undefined;
    if (next === state) return undefined;
    return { state: next, value: next, events: [next ? "on" : "off"] };
  },
});
