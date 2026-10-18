/**
 * The plugin in-process (SDK runtime), in simulate mode and in live mode
 * against the protocol-level fake Videohub.
 */

import { assert, assertEquals, assertRejects } from "@std/assert";
import type { ActionError } from "@covalence-io/sdk";
import { ConnectionRuntime } from "@covalence-io/sdk/runtime";
import plugin from "./mod.ts";
import { FakeVideohub } from "./fake_server.ts";

type Sent = [string, Record<string, unknown>];

function harness(mode: "live" | "simulate", config: Record<string, unknown>) {
  const sent: Sent[] = [];
  const waiters: (() => void)[] = [];
  const rt = new ConnectionRuntime(plugin, { alias: "vh", mode, config }, {
    notify(m, p) {
      sent.push([m, p as Record<string, unknown>]);
      waiters.splice(0).forEach((w) => w());
    },
  });
  /** Waits until a sent message matches. */
  async function until(match: (s: Sent) => boolean, ms = 3000): Promise<Sent> {
    const deadline = Date.now() + ms;
    for (;;) {
      const hit = sent.find(match);
      if (hit) return hit;
      if (Date.now() > deadline) {
        throw new Error(
          `timed out; sent: ${JSON.stringify(sent.map(([m]) => m))}`,
        );
      }
      await new Promise<void>((r) => {
        waiters.push(r);
        setTimeout(r, 50);
      });
    }
  }
  return { rt, sent, until };
}

const status = (s: string) => ([m, p]: Sent) =>
  m === "conn.status" && p.status === s;

Deno.test("manifest declares the route action, events and both simulators", () => {
  const m = plugin.manifest;
  assertEquals(m.id, "io.covalence.videohub");
  assertEquals(m.actions.map((a) => [a.id, a.idempotent]), [["route", true]]);
  assertEquals(m.events.map((e) => e.id), ["routeChanged"]);
  assertEquals(m.simulator, { stateLevel: true, protocolLevel: true });
  assertEquals(Object.keys(m.state), ["model", "inputs", "outputs"]);
});

Deno.test("simulate: routes, patches and emits once per change", async () => {
  const { rt, sent } = harness("simulate", {});
  assert(await rt.start());
  const snapshot = sent.find(([m]) => m === "state.snapshot")![1];
  const values = snapshot.values as { p: unknown[]; v: unknown }[];
  assertEquals(values.length, 1 + 12 + 24);
  assert(
    values.some((x) =>
      JSON.stringify(x) === '{"p":["outputs",1,"source"],"v":1}'
    ),
  );

  sent.length = 0;
  assertEquals(
    await rt.invoke({
      call: "c1",
      action: "route",
      params: { output: 1, input: 3 },
    }),
    {},
  );
  assertEquals(
    sent.map(([m, p]) => [m, m === "state.patch" ? p.ops : p.payload]),
    [
      ["state.patch", [{ p: ["outputs", 1, "source"], v: 3 }]],
      ["event.emit", { output: 1, input: 3 }],
    ],
  );

  sent.length = 0;
  await rt.invoke({
    call: "c2",
    action: "route",
    params: { output: 1, input: 3 },
  });
  assertEquals(sent, [], "idempotent: the same route changes nothing");

  const err = await assertRejects(() =>
    rt.invoke({ call: "c3", action: "route", params: { output: 99, input: 0 } })
  ) as ActionError;
  assertEquals(err.code, "invalid_params");
  await rt.destroy("test");
});

Deno.test("live: prelude becomes the snapshot; routes go through the device", async () => {
  const hub = new FakeVideohub({ inputs: 4, outputs: 2 });
  const { rt, sent, until } = harness("live", {
    host: hub.hostname,
    port: hub.port,
  });
  try {
    assert(await rt.start());
    assertEquals(sent.map(([m, p]) => m === "conn.status" ? p.status : m), [
      "connecting",
      "state.snapshot",
      "ok",
    ]);
    const values = sent[1][1].values as { p: unknown[]; v: unknown }[];
    assertEquals(values, [
      { p: ["model"], v: "Blackmagic Smart Videohub 4x2" },
      { p: ["inputs", 0, "name"], v: "Input 1" },
      { p: ["inputs", 1, "name"], v: "Input 2" },
      { p: ["inputs", 2, "name"], v: "Input 3" },
      { p: ["inputs", 3, "name"], v: "Input 4" },
      { p: ["outputs", 0, "name"], v: "Output 1" },
      { p: ["outputs", 0, "source"], v: 0 },
      { p: ["outputs", 1, "name"], v: "Output 2" },
      { p: ["outputs", 1, "source"], v: 1 },
    ]);

    sent.length = 0;
    await rt.invoke({
      call: "c1",
      action: "route",
      params: { output: 1, input: 3 },
    });
    assertEquals(hub.received, ["VIDEO OUTPUT ROUTING:\n1 3\n\n"]);
    assertEquals(hub.routes, [0, 3]);
    await until(([m]) => m === "event.emit");
    assertEquals(
      sent.map(([m, p]) => [m, m === "state.patch" ? p.ops : p.payload]),
      [
        ["state.patch", [{ p: ["outputs", 1, "source"], v: 3 }]],
        ["event.emit", { output: 1, input: 3 }],
      ],
    );

    // A change made at the device shows up too.
    sent.length = 0;
    await hub.route(0, 2);
    await until(([m]) => m === "event.emit");
    assertEquals(sent.at(-1)![1].payload, { output: 0, input: 2 });

    const err = await assertRejects(() =>
      rt.invoke({
        call: "c2",
        action: "route",
        params: { output: 2, input: 0 },
      })
    ) as ActionError;
    assertEquals(err.code, "invalid_params");
  } finally {
    await rt.destroy("test");
    await hub.close();
  }
});

Deno.test("live: a dropped link goes disconnected, reconnects and resnapshots", async () => {
  const hub = new FakeVideohub({ inputs: 2, outputs: 2 });
  const { rt, sent, until } = harness("live", {
    host: hub.hostname,
    port: hub.port,
  });
  try {
    assert(await rt.start());
    sent.length = 0;
    hub.dropClients();
    await until(status("disconnected"));
    hub.routes[0] = 1; // changed while we were away
    await until(status("ok"));
    const kinds = sent.map(([m, p]) => m === "conn.status" ? p.status : m);
    const ok = kinds.lastIndexOf("ok");
    assertEquals(kinds[ok - 1], "state.snapshot");
    const values = sent[ok - 1][1].values as { p: unknown[]; v: unknown }[];
    assert(
      values.some((x) =>
        JSON.stringify(x) === '{"p":["outputs",0,"source"],"v":1}'
      ),
    );
  } finally {
    await rt.destroy("test");
    await hub.close();
  }
});

Deno.test("live: an unreachable device reports error and actions fail not_connected", async () => {
  const hub = new FakeVideohub();
  const port = hub.port;
  await hub.close();
  const { rt, sent } = harness("live", { host: "127.0.0.1", port });
  try {
    assert(await rt.start());
    assertEquals(sent.at(-1)![1].status, "error");
    const err = await assertRejects(() =>
      rt.invoke({ call: "c", action: "route", params: { output: 0, input: 0 } })
    ) as ActionError;
    assertEquals(err.code, "not_connected");
  } finally {
    await rt.destroy("test");
  }
});
