import { assert, assertEquals, assertRejects } from "@std/assert";
import { ActionError } from "./plugin.ts";
import { type HidDevice, type IoSink, toBytes, Transports } from "./io.ts";

/** A pretend core: records what the transports send and answers requests. */
class FakeCore implements IoSink {
  sent: [string, unknown][] = [];
  next = 1;
  answer: (m: string, p: unknown) => unknown = (m) =>
    m === "io.open" ? { handle: `h${this.next++}` } : {};
  notify(m: string, p: unknown) {
    this.sent.push([m, p]);
  }
  request(m: string, p: unknown): Promise<unknown> {
    this.sent.push([m, p]);
    try {
      return Promise.resolve(this.answer(m, p));
    } catch (e) {
      return Promise.reject(e);
    }
  }
}

Deno.test("bytes arrive in either form", () => {
  assertEquals(toBytes([1, 2, 255]), new Uint8Array([1, 2, 255]));
  assertEquals(toBytes("hi"), new Uint8Array([104, 105]));
  const u = new Uint8Array([9]);
  assertEquals(toBytes(u), u);
});

Deno.test("a HID handle sends reports and hears input and state", async () => {
  const core = new FakeCore();
  core.answer = (m, p) => {
    if (m === "io.open") return { handle: "d1" };
    if (m === "io.feature" && (p as { op: string }).op === "get") {
      return { data: [6, 1, 2] };
    }
    return {};
  };
  const io = new Transports(core);
  const deck = await io.hid.open("hid:0fd9:0080:AL12") as HidDevice;
  assertEquals(core.sent[0], ["io.open", {
    kind: "hid",
    target: "hid:0fd9:0080:AL12",
  }]);

  const inputs: number[][] = [];
  const states: string[] = [];
  deck.onInput((d) => inputs.push([...d]));
  deck.onState((s, e) => states.push(e ? `${s}:${e}` : s));
  io.receive("io.data", { handle: "d1", data: [1, 0, 1] });
  io.receive("io.state", { handle: "d1", state: "closed", error: "unplugged" });
  io.receive("io.state", { handle: "d1", state: "open" });
  assertEquals(inputs, [[1, 0, 1]]);
  assertEquals(states, ["closed:unplugged", "open"]);
  assert(!deck.closed, "device handles outlive an unplug");

  deck.write([2, 7]);
  assertEquals(core.sent.at(-1), ["io.write", {
    handle: "d1",
    data: new Uint8Array([2, 7]),
  }]);
  assertEquals([...await deck.getFeatureReport(6, 3)], [6, 1, 2]);
  await deck.sendFeatureReport([3, 8, 50]);
  assertEquals((core.sent.at(-1)![1] as { op: string }).op, "send");

  io.closeAll();
  assertEquals(core.sent.at(-1), ["io.close", { handle: "d1" }]);
  assert(deck.closed);
});

Deno.test("events that beat io.open's answer are kept", async () => {
  const core = new FakeCore();
  const io = new Transports(core);
  // The core's io.state for h1 arrives first.
  io.receive("io.state", { handle: "h1", state: "closed", error: "unplugged" });
  const handle = await io.serial.open("serial@/dev/ttyACM0", { baud: 9600 });
  assertEquals(handle.state, "closed");
  assertEquals(handle.error, "unplugged");
  assertEquals((core.sent[0][1] as { options: unknown }).options, {
    baud: 9600,
  });
});

Deno.test("tcp.connect is shaped like a Deno.Conn", async () => {
  const core = new FakeCore();
  const io = new Transports(core);
  const conn = await io.tcp.connect({ port: 9990, reconnect: true });
  assertEquals(conn.remoteAddr, {
    transport: "tcp",
    hostname: "127.0.0.1",
    port: 9990,
  });
  assertEquals(core.sent[0][1], {
    kind: "tcp",
    target: "127.0.0.1:9990",
    options: { reconnect: true },
  });

  io.receive("io.data", { handle: "h1", data: [1, 2, 3, 4, 5] });
  const buf = new Uint8Array(3);
  assertEquals(await conn.read(buf), 3);
  assertEquals([...buf], [1, 2, 3]);
  assertEquals(await conn.read(buf), 2);
  assertEquals([...buf.subarray(0, 2)], [4, 5]);

  const reading = conn.read(buf);
  io.receive("io.state", { handle: "h1", state: "closed", error: "gone" });
  assertEquals(await reading, null, "closed: read gives null");

  // A refused first attempt rejects.
  core.answer = () => {
    throw new ActionError("not_connected", "refused");
  };
  await assertRejects(() => io.tcp.connect({ port: 1 }), ActionError);
});

Deno.test("tcp state changes keep their place among the bytes", async () => {
  const core = new FakeCore();
  const io = new Transports(core);
  // The first state beats the answer to the wrapper: it's replayed to it.
  core.answer = (m) => {
    if (m === "io.open") {
      io.receive("io.state", {
        handle: "h1",
        state: "reconnecting",
        error: "refused",
      });
      return { handle: "h1" };
    }
    return {};
  };
  const log: string[] = [];
  const conn = await io.tcp.connect({ port: 9990, reconnect: true });
  assertEquals(conn.handle.state, "reconnecting");
  conn.onState((s) => log.push(s));

  io.receive("io.state", { handle: "h1", state: "open" });
  io.receive("io.data", { handle: "h1", data: [1, 2] });
  io.receive("io.state", { handle: "h1", state: "reconnecting", error: "x" });
  io.receive("io.state", { handle: "h1", state: "open" });
  io.receive("io.data", { handle: "h1", data: [3] });
  assertEquals(log, [], "the first change waited for a listener and a read");

  const buf = new Uint8Array(1);
  assertEquals(await conn.read(buf), 1);
  assertEquals(log, ["reconnecting", "open"], "before [1, 2]");
  assertEquals(await conn.read(buf), 1);
  assertEquals(log, ["reconnecting", "open", "reconnecting", "open"]);
  assertEquals(await conn.read(buf), 1);
  assertEquals([...buf], [3]);

  // Nothing unread, and a listener: at once.
  io.receive("io.state", { handle: "h1", state: "reconnecting", error: "y" });
  assertEquals(log.at(-1), "reconnecting");
});

Deno.test("tcp readable and writable streams", async () => {
  const core = new FakeCore();
  const io = new Transports(core);
  const conn = await io.tcp.connect({ port: 1 });
  const writer = conn.writable.getWriter();
  await writer.write(new Uint8Array([7]));
  assertEquals(core.sent.at(-1)![0], "io.write");
  io.receive("io.data", { handle: "h1", data: [8, 9] });
  const reader = conn.readable.getReader();
  assertEquals([...(await reader.read()).value!], [8, 9]);
  io.receive("io.state", { handle: "h1", state: "closed" });
  assertEquals((await reader.read()).done, true);
});

Deno.test("net.socket is shaped like a node:net Socket", async () => {
  const core = new FakeCore();
  const io = new Transports(core);
  const socket = io.net.socket();
  socket.setEncoding("utf-8");
  socket.write("early"); // queued until connected
  const connected = new Promise<void>((resolve) =>
    socket.connect(9990, "10.0.0.12", resolve)
  );
  await connected;
  assertEquals(core.sent[0][1], { kind: "tcp", target: "10.0.0.12:9990" });
  assertEquals(core.sent[1], ["io.write", {
    handle: "h1",
    data: new TextEncoder().encode("early"),
  }]);
  // io.open answers once the core has connected.
  assertEquals(socket.readyState, "open");
  io.receive("io.state", { handle: "h1", state: "open" });

  const got: string[] = [];
  socket.on("data", (d: string) => got.push(d));
  io.receive("io.data", { handle: "h1", data: toBytes("hello") });
  assertEquals(got, ["hello"]);

  const closed = new Promise<boolean>((resolve) => socket.on("close", resolve));
  io.receive("io.state", {
    handle: "h1",
    state: "closed",
    error: "closed by the peer",
  });
  assertEquals(await closed, false);
  assert(socket.destroyed);
});

Deno.test("a socket that can't connect emits error and close", async () => {
  const core = new FakeCore();
  core.answer = () => {
    throw new ActionError("not_connected", "refused");
  };
  const io = new Transports(core);
  const socket = io.net.socket();
  const error = new Promise<Error>((resolve) => socket.on("error", resolve));
  const closed = new Promise<boolean>((resolve) => socket.on("close", resolve));
  socket.connect({ port: 1, host: "127.0.0.1" });
  assertEquals((await error).message, "refused");
  assertEquals(await closed, true);
});
