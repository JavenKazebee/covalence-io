// A value as text; `undefined` stays `undefined`.
export default defineTransform({
  id: "format",
  version: 1,
  kind: "stateless",
  options: {
    input: {},
    format: { type: "string", enum: ["auto", "mmss", "hhmmss", "percent", "fixed1", "fixed2", "upper"] },
  },
  result: { type: "string" },
  compute: ({ options: { input: v, format } }) => {
    if (v === undefined) return undefined;
    switch (format) {
      case "mmss": return mmss(v);
      case "hhmmss": return hhmmss(v);
      case "percent": return `${Math.round(Number(v) * 100)}%`;
      case "fixed1": return Number(v).toFixed(1);
      case "fixed2": return Number(v).toFixed(2);
      case "upper": return String(v).toUpperCase();
      default: return String(v);
    }
  },
});
