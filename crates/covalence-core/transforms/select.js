// An index moved by its input's changes, over a count or a list; gives the
// index, or the item.
const size = (options) =>
  Array.isArray(options.items)
    ? options.items.length
    : typeof options.count === "number"
      ? Math.max(Math.floor(options.count), 0)
      : 0;
const clamp = (i, n) => Math.min(Math.max(typeof i === "number" ? i : 0, 0), n - 1);
const at = (options, i) => (Array.isArray(options.items) ? options.items[i] : i);

export default defineTransform({
  id: "select",
  version: 1,
  kind: "reducer",
  options: {
    count: { type: "number" },
    items: { type: "array" },
    wrap: { type: "boolean", default: false },
    initial: { type: "number", default: 0 },
  },
  steps: { input: "number" },
  result: {},
  events: { select: { index: { type: "number" }, value: {} } },
  init: ({ options }) => options.initial,
  initValue: ({ state, options }) => {
    const n = size(options);
    return n > 0 ? at(options, clamp(state, n)) : undefined;
  },
  step: (state, { input, previous, options }) => {
    if (typeof input !== "number" || typeof previous !== "number") return undefined;
    const n = size(options);
    if (n === 0) return undefined;
    const from = clamp(state, n);
    let next = from + Math.round(input - previous);
    next = options.wrap ? ((next % n) + n) % n : Math.min(Math.max(next, 0), n - 1);
    if (next === state) return undefined;
    const value = at(options, next);
    return { state: next, value, events: [{ name: "select", payload: { index: next, value } }] };
  },
});
