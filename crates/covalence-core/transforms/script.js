// Whatever its own source returns. `source` is an expression or a function
// body, with `state`, `event`, `input`, `previous`, `value` and `now` in scope.
// It returns `{value, state?, events?, wakeAt?}`, a plain value (which becomes
// both the state and the value), or `undefined` for no change.
const params = ["state", "event", "input", "previous", "value", "now"];
const compile = (source) => {
  try {
    return new Function(...params, "return (\n" + source + "\n);");
  } catch (e) {
    return new Function(...params, source);
  }
};

export default defineTransform({
  id: "script",
  version: 1,
  kind: "reducer",
  options: {
    source: { type: "string" },
    initial: {},
  },
  steps: { on: ["*"], input: "any" },
  result: {},
  events: {},
  init: ({ options }) => options.initial,
  step: (state, { event, input, previous, value, now, options }) => {
    const r = compile(options.source)(state, event, input, previous, value, now);
    if (r === undefined) return undefined;
    const shaped = r !== null && typeof r === "object" && !Array.isArray(r)
      && ("value" in r || "state" in r || "events" in r || "wakeAt" in r);
    if (!shaped) return { state: r, value: r };
    return {
      state: "state" in r ? r.state : "value" in r ? r.value : state,
      value: "value" in r ? r.value : value,
      events: r.events,
      wakeAt: r.wakeAt,
    };
  },
});
