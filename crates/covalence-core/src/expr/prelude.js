// The sandbox around every expression (flow spec §8.7, ADR 0021). Evaluated in
// each fresh context; it returns `setup`, which Rust calls once with the
// natives and the scope, and which returns `out`, the result converter.
//
// Natives:
//   read(id, key) -> "m" (missing) | "b<id>" (a branch node) | "l<json>" (a leaf)
//   keys(id)      -> a JSON array of the keys directly below a node
(function (g) {
  "use strict";

  const RealDate = g.Date;
  const RealPromise = g.Promise;
  const freezeAll = (v) => {
    if (v !== null && typeof v === "object") {
      for (const x of Object.values(v)) freezeAll(x);
      Object.freeze(v);
    }
    return v;
  };
  const parse = (json) => freezeAll(JSON.parse(json));
  const constant = (name, value) =>
    Object.defineProperty(g, name, { value, writable: false, enumerable: false, configurable: false });

  const readOnly = () => {
    throw new TypeError("state is read-only here; use a `set` step");
  };
  const nondet = (what, instead) => () => {
    throw new TypeError(`${what} isn't available in expressions; ${instead}`);
  };

  // Time and randomness would make replay inexact, and an expression that
  // read the clock would never re-evaluate. `sys.time.now` is state.
  function Date(...args) {
    if (!new.target) nondet("Date()", "read sys.time.now instead")();
    if (args.length === 0) nondet("new Date()", "pass a timestamp, such as sys.time.now")();
    return Reflect.construct(RealDate, args, new.target);
  }
  Date.prototype = RealDate.prototype;
  Date.UTC = RealDate.UTC;
  Date.parse = RealDate.parse;
  Date.now = nondet("Date.now()", "read sys.time.now instead");
  RealDate.prototype.constructor = Date;
  g.Date = Date;
  Math.random = nondet("Math.random()", "randomness belongs in a script step");
  // An expression returns its value straight away; waiting is for steps.
  delete g.Promise;

  // The standard library (ADR 0004).
  const two = (n) => String(n).padStart(2, "0");
  const clock = (seconds, withHours) => {
    const sign = seconds < 0 ? "-" : "";
    let s = Math.floor(Math.abs(seconds));
    const h = Math.floor(s / 3600);
    const m = withHours ? Math.floor((s % 3600) / 60) : Math.floor(s / 60);
    s %= 60;
    return withHours ? `${sign}${two(h)}:${two(m)}:${two(s)}` : `${sign}${two(m)}:${two(s)}`;
  };
  const clamp = (x, lo, hi) => Math.min(Math.max(x, lo), hi);
  const byte = (x) => clamp(Math.round(x), 0, 255);
  const hex = (r, gr, b) => "#" + [r, gr, b].map((x) => byte(x).toString(16).padStart(2, "0")).join("");
  const channels = (color) => {
    let h = String(color).replace(/^#/, "");
    if (h.length === 3) h = [...h].map((c) => c + c).join("");
    if (!/^[0-9a-fA-F]{6}$/.test(h)) throw new TypeError(`not a colour: ${color}`);
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16));
  };
  const library = {
    mmss: (seconds) => clock(seconds, false),
    hhmmss: (seconds) => clock(seconds, true),
    clamp,
    round: (x, digits = 0) => Math.round(x * 10 ** digits) / 10 ** digits,
    pad: (x, width, fill = "0") => String(x).padStart(width, fill),
    lerp: (a, b, t) => a + (b - a) * t,
    rgb: (r, gr, b) => hex(r, gr, b),
    hsl: (h, s, l) => {
      s = clamp(s, 0, 100) / 100;
      l = clamp(l, 0, 100) / 100;
      const k = (n) => (n + h / 30) % 12;
      const a = s * Math.min(l, 1 - l);
      const f = (n) => l - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1));
      return hex(f(0) * 255, f(8) * 255, f(4) * 255);
    },
    mix: (a, b, t) => {
      const x = channels(a);
      const y = channels(b);
      return hex(...x.map((c, i) => c + (y[i] - c) * t));
    },
  };
  for (const [name, f] of Object.entries(library)) constant(name, f);

  // Values out of an expression, as JSON, or `undefined` (flow spec §8.7).
  const plain = (v, depth) => {
    if (depth > 64) throw new TypeError("the result is nested too deeply, or refers to itself");
    switch (typeof v) {
      case "undefined":
      case "boolean":
      case "string":
        return v;
      case "number":
        if (!Number.isFinite(v)) throw new TypeError(`${v} can't be a value`);
        return v;
      case "bigint":
        if (v > BigInt(Number.MAX_SAFE_INTEGER) || v < BigInt(Number.MIN_SAFE_INTEGER)) {
          throw new TypeError(`${v} is too large to be a value`);
        }
        return Number(v);
      case "object": {
        if (v === null) return null;
        if (v instanceof RealDate) return v.getTime();
        if (v instanceof RealPromise) throw new TypeError("an expression can't return a promise; it can't wait");
        if (Array.isArray(v)) return v.map((x) => plain(x, depth + 1) ?? null);
        const o = {};
        for (const k of Object.keys(v)) {
          const x = plain(v[k], depth + 1);
          if (x !== undefined) o[k] = x;
        }
        return o;
      }
      default:
        throw new TypeError(`a ${typeof v} can't be a value`);
    }
  };
  const out = (v) => {
    const p = plain(v, 0);
    return p === undefined ? undefined : JSON.stringify(p);
  };

  return function setup(read, keys, roots, trigger, local, steps) {
    // A read-only view of a state node. Children are cached, so the same path
    // gives the same object within one evaluation.
    const node = (id) => {
      const cache = new Map();
      const child = (k) => {
        if (cache.has(k)) return cache.get(k);
        const r = read(id, k);
        let v;
        if (r[0] === "b") v = node(Number(r.slice(1)));
        else if (r[0] === "l") v = parse(r.slice(1));
        else v = undefined;
        cache.set(k, v);
        return v;
      };
      return new Proxy(
        {},
        {
          get: (_, k) => {
            if (typeof k === "symbol") return undefined;
            const v = child(k);
            // `String(node)` and friends still work on a key that isn't state.
            return v === undefined && k in Object.prototype ? Object.prototype[k] : v;
          },
          has: (_, k) => typeof k !== "symbol" && child(k) !== undefined,
          ownKeys: () => JSON.parse(keys(id)),
          getOwnPropertyDescriptor: (_, k) => {
            if (typeof k === "symbol") return undefined;
            const value = child(k);
            return value === undefined
              ? undefined
              : { value, writable: false, enumerable: true, configurable: true };
          },
          set: readOnly,
          defineProperty: readOnly,
          deleteProperty: readOnly,
          setPrototypeOf: readOnly,
          preventExtensions: readOnly,
        },
      );
    };

    constant("trigger", trigger === undefined ? undefined : parse(trigger));
    constant("local", parse(local));
    constant("steps", parse(steps));
    // An alias that is already a global name (`Math`, `clamp`, `steps`) is
    // left out rather than hiding the global.
    for (const [name, id] of Object.entries(JSON.parse(roots))) {
      if (!(name in g)) constant(name, node(id));
    }
    return out;
  };
})(globalThis);
