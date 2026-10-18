/**
 * End to end: a fake core spawns the real host process and drives it over
 * stdio, with the Videohub plugin in simulate mode and in live mode against
 * the fake Videohub server.
 */

import { assert, assertEquals, assertRejects } from "@std/assert";
import type { Manifest, Notification } from "@covalence-io/protocol";
import { FakeVideohub } from "@covalence-io/plugin-videohub/fake-server";
import { CoreError, FakeCore, REPO_ROOT } from "./fake_core.ts";
import { toUrl } from "./host.ts";

const VIDEOHUB = `${REPO_ROOT}/plugins/videohub/mod.ts`;
const TEST_PLUGIN = `${REPO_ROOT}/packages/plugin-host/testdata/test_plugin.ts`;

const create = (
  plugin: string,
  version: string,
  mode: string,
  config = {},
) => ({
  plugin,
  version,
  alias: "x",
  config,
  secrets: {},
  permissions: {},
  mode,
  isolate: false,
});

const is = (conn: string, m: string, status?: string) => (n: Notification) =>
  n.conn === conn && n.m === m &&
  (status === undefined || (n.p as { status?: string }).status === status);

async function code(p: Promise<unknown>): Promise<string> {
  const err = await assertRejects(() => p, CoreError);
  return (err as CoreError).error.code;
}

async function loadVideohub(core: FakeCore) {
  const r = await core.request("plugin.load", {
    plugin: "io.covalence.videohub",
    version: "0.1.0",
    bundle: VIDEOHUB,
  }) as { manifest: Manifest };
  return r.manifest;
}

Deno.test("simulate: hello, load, create, route, resync, destroy", async () => {
  const core = new FakeCore({ term: 3 });
  try {
    await core.helloSeen;
    assertEquals(core.hello?.host.kind, "deno-shared");
    assertEquals(core.hello?.protocol.min, "0.1.0");
    assert(core.hello?.capabilities.includes("workers"));

    const manifest = await loadVideohub(core);
    assertEquals(manifest.id, "io.covalence.videohub");
    assertEquals(manifest.actions[0].id, "route");

    const conn = "01JSIMULATED00000000000000";
    assertEquals(
      await core.request(
        "conn.create",
        create("io.covalence.videohub", "0.1.0", "simulate"),
        conn,
      ),
      {},
    );
    await core.waitFor(is(conn, "conn.status", "ok"));
    const ours = core.notifications.filter((n) => n.conn === conn);
    assertEquals(ours.map((n) => n.m), [
      "conn.status",
      "state.snapshot",
      "conn.status",
    ]);
    assertEquals((ours[0].p as { status: string }).status, "connecting");

    const mark = core.notifications.length;
    const result = await core.request("action.invoke", {
      call: "01JCALL0000000000000000001",
      term: 3,
      action: "route",
      params: { output: 1, input: 3 },
      deadline: 2000,
    }, conn);
    assertEquals(result, {});
    const patch = await core.waitFor(is(conn, "state.patch"), { from: mark });
    assertEquals((patch.p as { ops: unknown }).ops, [
      { p: ["outputs", 1, "source"], v: 3 },
    ]);
    const event = await core.waitFor(is(conn, "event.emit"), { from: mark });
    assertEquals((event.p as { event: string }).event, "routeChanged");
    assertEquals((event.p as { payload: unknown }).payload, {
      output: 1,
      input: 3,
    });

    // Fencing: a term below the highest seen (3, from host.welcome) fails.
    assertEquals(
      await code(core.request("action.invoke", {
        call: "01JCALL0000000000000000002",
        term: 2,
        action: "route",
        params: { output: 1, input: 0 },
      }, conn)),
      "stale_term",
    );
    assertEquals(
      await code(core.request("action.invoke", {
        call: "01JCALL0000000000000000003",
        term: 3,
        action: "route",
        params: { output: 1 },
      }, conn)),
      "invalid_params",
    );

    const before = core.notifications.length;
    assertEquals(await core.request("conn.resync", {}, conn), {});
    const snap = await core.waitFor(is(conn, "state.snapshot"), {
      from: before,
    });
    const values = (snap.p as { values: { p: unknown[]; v: unknown }[] })
      .values;
    assertEquals(
      values.find((x) => JSON.stringify(x.p) === '["outputs",1,"source"]')?.v,
      3,
    );

    assertEquals(await core.request("ping", {}), {});
    assertEquals(await code(core.request("nope", {})), "unknown_method");
    assertEquals(
      await core.request("conn.destroy", { reason: "test" }, conn),
      {},
    );
    assertEquals(
      await code(core.request("conn.resync", {}, conn)),
      "not_found",
    );
  } finally {
    const { code } = await core.close();
    assertEquals(code, 0);
  }
});

Deno.test("live (JSON debug mode): routes through the fake Videohub", async () => {
  const hub = new FakeVideohub({ inputs: 8, outputs: 4 });
  const core = new FakeCore({ format: "json" });
  try {
    await core.helloSeen;
    await loadVideohub(core);
    const conn = "01JLIVE0000000000000000000";
    await core.request(
      "conn.create",
      create("io.covalence.videohub", "0.1.0", "live", {
        host: hub.hostname,
        port: hub.port,
      }),
      conn,
    );
    await core.waitFor(is(conn, "conn.status", "ok"));
    const snap = await core.waitFor(is(conn, "state.snapshot"));
    const values = (snap.p as { values: { p: unknown[]; v: unknown }[] })
      .values;
    assertEquals(values[0], {
      p: ["model"],
      v: "Blackmagic Smart Videohub 8x4",
    });

    let mark = core.notifications.length;
    await core.request("action.invoke", {
      call: "01JCALL0000000000000000010",
      term: 1,
      action: "route",
      params: { output: 1, input: 3 },
    }, conn);
    assertEquals(hub.routes[1], 3);
    const patch = await core.waitFor(is(conn, "state.patch"), { from: mark });
    assertEquals((patch.p as { ops: unknown }).ops, [
      { p: ["outputs", 1, "source"], v: 3 },
    ]);
    await core.waitFor(is(conn, "event.emit"), { from: mark });

    // A change made at the device.
    mark = core.notifications.length;
    await hub.route(2, 7);
    const event = await core.waitFor(is(conn, "event.emit"), { from: mark });
    assertEquals((event.p as { payload: unknown }).payload, {
      output: 2,
      input: 7,
    });

    await core.request("conn.destroy", { reason: "test" }, conn);
  } finally {
    await core.close();
    await hub.close();
  }
});

Deno.test("deadlines, worker crashes and console output stay contained", async () => {
  const core = new FakeCore();
  try {
    await core.helloSeen;
    await core.request("plugin.load", {
      plugin: "io.covalence.test",
      version: "0.0.1",
      bundle: toUrl(TEST_PLUGIN),
    });
    const a = "01JCONNA000000000000000000";
    const b = "01JCONNB000000000000000000";
    for (const conn of [a, b]) {
      await core.request(
        "conn.create",
        create("io.covalence.test", "0.0.1", "simulate"),
        conn,
      );
      await core.waitFor(is(conn, "conn.status", "ok"));
    }
    // The plugin's console.log arrived as a log notification.
    await core.waitFor((n) =>
      n.conn === a && n.m === "log" &&
      (n.p as { msg: string }).msg === "hello from the plugin"
    );

    assertEquals(
      await code(core.request("action.invoke", {
        call: "01JCALL0000000000000000020",
        term: 1,
        action: "sleep",
        params: { ms: 10_000 },
        deadline: 100,
      }, a)),
      "timeout",
    );

    // Crash connection a's worker; b keeps working.
    await core.request("action.invoke", {
      call: "01JCALL0000000000000000021",
      term: 1,
      action: "crash",
      params: {},
    }, a);
    await core.waitFor(is(a, "conn.status", "error"));
    assertEquals(
      await code(core.request("action.invoke", {
        call: "01JCALL0000000000000000022",
        term: 1,
        action: "sleep",
        params: { ms: 1 },
      }, a)),
      "not_connected",
    );
    assertEquals(
      await core.request("action.invoke", {
        call: "01JCALL0000000000000000023",
        term: 1,
        action: "sleep",
        params: { ms: 1 },
      }, b),
      {},
    );

    assertEquals(
      await code(core.request(
        "conn.create",
        create("io.covalence.missing", "1.0.0", "simulate"),
        "01JCONNC000000000000000000",
      )),
      "not_found",
    );
    assertEquals(
      await code(core.request("plugin.load", {
        plugin: "io.covalence.other",
        version: "0.0.1",
        bundle: TEST_PLUGIN,
      })),
      "invalid_params",
    );
  } finally {
    const { code } = await core.close();
    assertEquals(code, 0);
  }
});

Deno.test("pipelined requests wait for plugin.load and conn.create", async () => {
  const core = new FakeCore();
  try {
    await core.helloSeen;
    const conn = "01JPIPELINED00000000000000";
    // Sent back to back, without waiting for each result.
    const results = await Promise.all([
      core.request("plugin.load", {
        plugin: "io.covalence.videohub",
        version: "0.1.0",
        bundle: VIDEOHUB,
      }),
      core.request(
        "conn.create",
        create("io.covalence.videohub", "0.1.0", "simulate"),
        conn,
      ),
      core.request("action.invoke", {
        call: "01JCALL0000000000000000030",
        term: 1,
        action: "route",
        params: { output: 0, input: 5 },
      }, conn),
    ]);
    assertEquals(results.slice(1), [{}, {}]);
    await core.waitFor(is(conn, "event.emit"));
  } finally {
    await core.close();
  }
});

Deno.test("host.shutdown answers, then the host exits", async () => {
  const core = new FakeCore();
  await core.helloSeen;
  assertEquals(await core.request("host.shutdown", {}), {});
  const { code } = await core.close();
  assertEquals(code, 0);
});

Deno.test("toUrl turns bundle paths into import specifiers", () => {
  assertEquals(toUrl("/a b/mod.ts"), "file:///a%20b/mod.ts");
  assertEquals(toUrl("file:///x.ts"), "file:///x.ts");
  assertEquals(toUrl("C:\\p\\mod.ts"), "file:///C:/p/mod.ts");
});
