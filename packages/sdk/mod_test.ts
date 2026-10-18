import { assertEquals, assertRejects } from "@std/assert";
import { ActionError, definePlugin, PROTOCOL_VERSION, s } from "./mod.ts";
import { ConnectionRuntime } from "./runtime.ts";

const plugin = definePlugin({
  id: "io.example.toy",
  name: "Toy",
  version: "1.2.3",
  config: s.object({ host: s.string(), port: s.integer({ default: 1 }) }),
  permissions: { net: ["{config.host}:{config.port}"] },
  state: { level: s.number({ unit: "dB" }) },
  events: { bumped: s.object({ level: s.number() }) },
  connect(ctx) {
    ctx.state.set({ level: 0 });
    return { host: ctx.config.host };
  },
  actions: {
    bump: {
      name: "Bump",
      params: s.object({ by: s.number() }),
      run(ctx, { by }) {
        const level = (ctx.state.get("level") as number) + by;
        ctx.state.patch("level", level);
        ctx.emit("bumped", { level });
        return ctx.session.host;
      },
    },
    fail: {
      name: "Fail",
      params: s.object({}),
      run() {
        throw new ActionError("not_connected", "offline");
      },
    },
  },
  simulator: {
    init: (sim) => sim.state.set("level", -10),
    actions: { bump: (sim, { by }) => sim.state.patch("level", -10 + by) },
  },
});

function recorder() {
  const sent: [string, unknown][] = [];
  return {
    sent,
    sink: { notify: (m: string, p: unknown) => sent.push([m, p]) },
  };
}

const strip = (sent: [string, unknown][]) =>
  sent.map(([m, p]) => {
    const { ts: _ts, ...rest } = p as Record<string, unknown>;
    return [m, rest];
  });

Deno.test("re-exports the protocol version", () => {
  assertEquals(PROTOCOL_VERSION, "0.1.0");
});

Deno.test("definePlugin builds the manifest", () => {
  const m = plugin.manifest;
  assertEquals(m.id, "io.example.toy");
  assertEquals(m.protocol, "^0.1");
  assertEquals(m.runtime, "deno");
  assertEquals(m.license, "MIT");
  assertEquals(m.config.version, 1);
  assertEquals(m.permissions, { net: ["{config.host}:{config.port}"] });
  assertEquals(m.actions.map((a) => a.id), ["bump", "fail"]);
  assertEquals(m.events, [{
    id: "bumped",
    name: "bumped",
    payload: s.object({ level: s.number() }),
  }]);
  assertEquals(m.simulator, { stateLevel: true, protocolLevel: false });
  assertEquals(structuredClone(m), m);
});

Deno.test("live: connecting status, snapshot, ok, then patch before event and result", async () => {
  const { sent, sink } = recorder();
  const rt = new ConnectionRuntime(plugin, {
    alias: "toy",
    mode: "live",
    config: { host: "h" },
  }, sink);
  assertEquals(await rt.start(), true);
  assertEquals(strip(sent), [
    ["conn.status", { status: "connecting" }],
    ["state.snapshot", { values: [{ p: ["level"], v: 0 }], complete: true }],
    ["conn.status", { status: "ok" }],
  ]);
  sent.length = 0;
  const res = await rt.invoke({
    call: "c1",
    action: "bump",
    params: { by: 2 },
  });
  assertEquals(res, { result: "h" });
  assertEquals(strip(sent), [
    ["state.patch", { ops: [{ p: ["level"], v: 2 }] }],
    ["event.emit", { event: "bumped", payload: { level: 2 } }],
  ]);
  await rt.destroy("test");
});

Deno.test("live: invalid config reports an error status", async () => {
  const { sent, sink } = recorder();
  const rt = new ConnectionRuntime(plugin, {
    alias: "toy",
    mode: "live",
    config: {},
  }, sink);
  assertEquals(await rt.start(), false);
  assertEquals(strip(sent).at(-1), ["conn.status", {
    status: "error",
    message: "config.host is required",
  }]);
  await assertRejects(
    () => rt.invoke({ call: "c", action: "bump", params: { by: 1 } }),
    ActionError,
    "not connected",
  );
});

Deno.test("action errors carry their code", async () => {
  const { sink } = recorder();
  const rt = new ConnectionRuntime(plugin, {
    alias: "toy",
    mode: "live",
    config: { host: "h" },
  }, sink);
  const code = (p: Promise<unknown>) =>
    p.then(() => "ok", (e: ActionError) => e.code);
  assertEquals(
    await code(rt.invoke({ call: "c", action: "fail", params: {} })),
    "not_connected",
  );
  assertEquals(
    await code(rt.invoke({ call: "c", action: "nope", params: {} })),
    "not_found",
  );
  assertEquals(
    await code(rt.invoke({ call: "c", action: "bump", params: {} })),
    "invalid_params",
  );
  await rt.destroy("test");
});

Deno.test("simulate: uses the simulator hooks and coalesces patches", async () => {
  const { sent, sink } = recorder();
  const rt = new ConnectionRuntime(plugin, {
    alias: "toy",
    mode: "simulate",
    config: {},
  }, sink);
  await rt.start();
  assertEquals(strip(sent)[1], ["state.snapshot", {
    values: [{ p: ["level"], v: -10 }],
    complete: true,
  }]);
  sent.length = 0;
  await rt.invoke({ call: "c", action: "bump", params: { by: 5 } });
  assertEquals(strip(sent), [["state.patch", {
    ops: [{ p: ["level"], v: -5 }],
  }]]);

  sent.length = 0;
  for (let i = 0; i < 5; i++) rt.context.state.patch("level", i);
  rt.context.state.patch("other", 1);
  await new Promise((r) => setTimeout(r, 10));
  assertEquals(strip(sent), [["state.patch", {
    ops: [{ p: ["level"], v: 4 }, { p: ["other"], v: 1 }],
  }]]);

  sent.length = 0;
  rt.resync();
  assertEquals(sent.length, 1);
  assertEquals(sent[0][0], "state.snapshot");
  await rt.destroy("test");
  rt.context.emit("bumped", { level: 1 });
  assertEquals(sent.length, 1, "nothing is sent after destroy");
});
