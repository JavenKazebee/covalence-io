// Units per second from its input's changes, decaying to 0 when they stop.
export default defineTransform({
  id: "velocity",
  version: 1,
  kind: "reducer",
  options: {
    // Changes within this many milliseconds count towards the rate.
    window: { type: "number", default: 100 },
    // The rate halves every this many milliseconds once the changes stop.
    decay: { type: "number", default: 250 },
    scale: { type: "number", default: 1 },
  },
  steps: { input: "number" },
  result: { type: "number" },
  events: {},
  init: () => ({ samples: [], rate: 0, at: 0 }),
  initValue: () => 0,
  step: (state, { input, previous, options, now }) => {
    const s = state !== null && typeof state === "object" && Array.isArray(state.samples)
      ? state
      : { samples: [], rate: 0, at: now };
    const samples = s.samples.filter((sample) => now - sample[0] < options.window);
    if (typeof input === "number" && typeof previous === "number" && input !== previous) {
      samples.push([now, input - previous]);
    }
    let sum = 0;
    for (const sample of samples) sum += sample[1];
    const raw = samples.length > 0 ? ((sum * 1000) / options.window) * options.scale : 0;
    const decayed = options.decay > 0 ? s.rate * Math.pow(0.5, (now - s.at) / options.decay) : 0;
    let rate = Math.abs(raw) >= Math.abs(decayed) ? raw : decayed;
    if (Math.abs(rate) < 0.001) rate = 0;
    const out = { state: { samples, rate, at: now }, value: rate };
    if (rate !== 0 || samples.length > 0) out.wakeAt = now + 20;
    return out;
  },
});
