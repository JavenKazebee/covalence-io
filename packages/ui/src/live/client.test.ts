import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useLiveStore } from "../stores/live";
import { LiveClient, type Socket } from "./client";
import type { ClientMessage, Leaf, ServerMessage } from "./protocol";

class FakeSocket implements Socket {
  readyState = 0;
  sent: ClientMessage[] = [];
  onopen: Socket["onopen"] = null;
  onclose: Socket["onclose"] = null;
  onmessage: Socket["onmessage"] = null;
  send(data: string) {
    this.sent.push(JSON.parse(data) as ClientMessage);
  }
  close() {
    this.readyState = 3;
  }
  open() {
    this.readyState = 1;
    this.onopen?.(new Event("open"));
  }
  drop() {
    this.readyState = 3;
    this.onclose?.({} as CloseEvent);
  }
  deliver(message: ServerMessage) {
    this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent);
  }
}

const leaf = (path: string, value: unknown): Leaf => ({ path, value, quality: "good", ts: 0, seq: 1 });

describe("live client", () => {
  let sockets: FakeSocket[];
  let client: LiveClient;
  let store: ReturnType<typeof useLiveStore>;
  const socket = () => sockets[sockets.length - 1]!;

  beforeEach(() => {
    vi.useFakeTimers();
    // No jitter, so the delays below are exact.
    vi.spyOn(Math, "random").mockReturnValue(1);
    setActivePinia(createPinia());
    store = useLiveStore();
    sockets = [];
    client = new LiveClient(store, {
      url: "ws://test/ws",
      connect: () => {
        const s = new FakeSocket();
        sockets.push(s);
        return s;
      },
      minBackoffMs: 100,
      maxBackoffMs: 1000,
    });
    client.start();
  });

  afterEach(() => {
    client.stop();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("subscribes once per path and unsubscribes after the last holder", () => {
    socket().open();
    client.subscribe("vh.outputs");
    client.subscribe("vh.outputs");
    client.unsubscribe("vh.outputs");
    client.unsubscribe("vh.outputs");
    expect(socket().sent).toEqual([
      { op: "subscribe", paths: ["vh.outputs"] },
      { op: "unsubscribe", paths: ["vh.outputs"] },
    ]);
  });

  it("drops leaves after unsubscribing unless another subscription covers them", () => {
    socket().open();
    client.subscribe("vh");
    client.subscribe("vh.outputs");
    socket().deliver({ type: "snapshot", paths: ["vh"], leaves: [leaf("vh.outputs[0].source", 1), leaf("vh.level", 2)] });
    client.unsubscribe("vh");
    expect([...store.leaves.keys()]).toEqual(["vh.outputs[0].source"]);
  });

  it("ignores a snapshot for a path unsubscribed while it was in flight", () => {
    socket().open();
    client.subscribe("vh.outputs");
    client.unsubscribe("vh.outputs");
    socket().deliver({ type: "snapshot", paths: ["vh.outputs"], leaves: [leaf("vh.outputs[0].source", 1)] });
    socket().deliver({ type: "patch", set: [leaf("vh.outputs[0].source", 2)], deleted: [] });
    expect(store.leaves.size).toBe(0);
  });

  it("reconnects with backoff and resubscribes everything held", () => {
    socket().open();
    client.subscribe("vh.outputs");
    client.subscribe("vh.inputs");
    const unfollow = client.followJournal({ entry: () => {} });
    socket().deliver({ type: "host", host: { phase: "ready", generation: 1, pid: 42 } });
    socket().drop();
    expect(store.connected).toBe(false);
    expect(store.host).toBeNull();

    // Subscribing while closed waits for the reconnect.
    client.subscribe("sys");
    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(100);
    expect(sockets).toHaveLength(2);
    socket().drop();
    vi.advanceTimersByTime(150);
    expect(sockets).toHaveLength(2);
    vi.advanceTimersByTime(50);
    expect(sockets).toHaveLength(3);

    socket().open();
    expect(store.connected).toBe(true);
    expect(socket().sent).toEqual([
      { op: "subscribe", paths: ["vh.outputs", "vh.inputs", "sys"] },
      { op: "journal", on: true },
    ]);
    unfollow();
    expect(socket().sent.at(-1)).toEqual({ op: "journal", on: false });
  });

  it("feeds host status, journal entries and errors", () => {
    socket().open();
    const entries: number[] = [];
    let missed = 0;
    client.followJournal({ entry: (e) => entries.push(e.seq), lagged: (n) => (missed = n) });
    socket().deliver({ type: "host", host: { phase: "ready", generation: 1, pid: 42 } });
    socket().deliver({ type: "journal", on: true });
    socket().deliver({ type: "journal", entry: { seq: 7, ts: "", kind: "log" } });
    socket().deliver({ type: "lagged", feed: "journal", missed: 3 });
    vi.spyOn(console, "warn").mockImplementation(() => {});
    socket().deliver({ type: "error", message: "bad path" });
    expect(store.host?.phase).toBe("ready");
    expect(entries).toEqual([7]);
    expect(missed).toBe(3);
    expect(store.lastError).toBe("bad path");
  });
});
