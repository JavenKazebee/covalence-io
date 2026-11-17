// A boolean flipped on each event (dataflow §4.2).
export default defineTransform({
  id: "toggle",
  version: 1,
  kind: "reducer",
  options: { initial: { type: "boolean", default: false } },
  steps: { on: ["press"] },
  result: { type: "boolean" },
  events: { on: {}, off: {} },
  init: ({ options }) => options.initial === true,
  step: (state, { event }) => {
    if (event === undefined) return undefined;
    const next = state !== true;
    return { state: next, value: next, events: [next ? "on" : "off"] };
  },
});
