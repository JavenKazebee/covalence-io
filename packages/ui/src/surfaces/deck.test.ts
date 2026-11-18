import { describe, expect, it, vi } from "vitest";
import type { Socket } from "../live/client";
import { DeckClient, faderPosition, newDeckState } from "./deck";
import { models, type VirtualLayout } from "./model";

class FakeSocket implements Socket {
  readyState = 0;
  sent: unknown[] = [];
  onopen: ((ev: Event) => unknown) | null = null;
  onclose: ((ev: CloseEvent) => unknown) | null = null;
  onmessage: ((ev: MessageEvent) => unknown) | null = null;
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.readyState = 3;
  }
  open() {
    this.readyState = 1;
    this.onopen?.({} as Event);
  }
  receive(message: unknown) {
    this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent);
  }
  closed(code: number, reason = "") {
    this.readyState = 3;
    this.onclose?.({ code, reason } as CloseEvent);
  }
}

function setup(layout?: VirtualLayout) {
  const sockets: FakeSocket[] = [];
  const state = newDeckState();
  let now = 100;
  const deck = new DeckClient(
    state,
    { id: "01S", slug: "foh", name: "FOH deck" },
    { id: "virtual.deck15", model: models["virtual.deck15"]!, layout },
    {
      url: "ws://test/surface/01S",
      connect: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
      now: () => now,
      minBackoffMs: 10,
    },
  );
  return { deck, state, sockets, tick: (ms: number) => (now += ms) };
}

describe("virtual deck", () => {
  it("attaches as a 5 × 3 deck of 144 px keys", () => {
    const { deck, state, sockets } = setup();
    deck.start();
    expect(state.status).toBe("connecting");
    sockets[0]!.open();
    expect(state.status).toBe("attached");
    const attach = sockets[0]!.sent[0] as { method: string; params: { controls: { id: string; x: number; y: number }[] } };
    expect(attach).toMatchObject({
      method: "surface.attach",
      params: { surface: "foh", model: "virtual.deck15", name: "FOH deck" },
    });
    expect(attach.params.controls).toHaveLength(15);
    expect(attach.params.controls[7]).toEqual({
      id: "k7",
      at: { x: 2, y: 1 },
      inputs: [{ kind: "momentary" }],
      outputs: [{ kind: "image", w: 144, h: 144, format: "png" }],
    });
  });

  it("shows what the core draws", () => {
    const { deck, state, sockets } = setup();
    deck.start();
    sockets[0]!.open();
    sockets[0]!.receive({ method: "surface.draw", params: { surface: "foh", control: "k3", format: "png", image: "AAAA" } });
    expect(state.images.k3).toBe("data:image/png;base64,AAAA");
  });

  it("sends down and up once each, with the browser's timestamps", () => {
    const { deck, state, sockets, tick } = setup();
    deck.start();
    sockets[0]!.open();
    deck.down("k0");
    deck.down("k0");
    tick(80);
    deck.up("k0");
    deck.up("k0");
    expect(sockets[0]!.sent.slice(1)).toEqual([
      { method: "surface.input", params: { surface: "foh", control: "k0", kind: "down", ts: 100 } },
      { method: "surface.input", params: { surface: "foh", control: "k0", kind: "up", ts: 180 } },
    ]);
    expect(state.down).toEqual({});
  });

  it("sends nothing until attached", () => {
    const { deck, state, sockets } = setup();
    deck.start();
    deck.down("k0");
    expect(sockets[0]!.sent).toEqual([]);
    expect(state.down).toEqual({});
  });

  it("reattaches after the connection drops, and lets go of held keys", () => {
    vi.useFakeTimers();
    try {
      const { deck, state, sockets } = setup();
      deck.start();
      sockets[0]!.open();
      deck.down("k1");
      sockets[0]!.closed(1006);
      expect(state).toMatchObject({ status: "closed", retrying: true, down: {} });
      vi.advanceTimersByTime(10);
      expect(sockets).toHaveLength(2);
      sockets[1]!.open();
      expect(state.status).toBe("attached");
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops when attached elsewhere or refused, rather than fight", () => {
    vi.useFakeTimers();
    try {
      for (const [code, reason] of [[1000, "attached elsewhere"], [1008, "cannot attach: no"]] as const) {
        const { deck, state, sockets } = setup();
        deck.start();
        sockets[0]!.open();
        sockets[0]!.closed(code, reason);
        vi.advanceTimersByTime(60_000);
        expect(sockets).toHaveLength(1);
        expect(state).toMatchObject({ status: "closed", retrying: false, reason });
      }
    } finally {
      vi.useRealTimers();
    }
  });

  it("attaches encoders beneath the keys and faders beside them", () => {
    const { deck, sockets } = setup({ encoders: 2, faders: 1 });
    deck.start();
    sockets[0]!.open();
    const controls = (sockets[0]!.sent[0] as { params: { controls: { id: string }[] } }).params.controls;
    expect(controls.slice(15)).toEqual([
      { id: "e0", at: { x: 0, y: 3 }, inputs: [{ kind: "relative" }, { kind: "momentary" }], outputs: [{ kind: "level" }] },
      { id: "e1", at: { x: 1, y: 3 }, inputs: [{ kind: "relative" }, { kind: "momentary" }], outputs: [{ kind: "level" }] },
      { id: "f0", at: { x: 5, y: 0, h: 3 }, inputs: [{ kind: "absolute" }, { kind: "touch" }], outputs: [{ kind: "level", motor: true }] },
    ]);
  });

  it("turns encoders and reports a fader only while it's touched", () => {
    const { deck, state, sockets } = setup({ encoders: 1, faders: 1 });
    deck.start();
    sockets[0]!.open();
    deck.turn("e0", -2);
    deck.move("f0", 0.4);
    deck.touch("f0", true);
    deck.move("f0", 0.4);
    deck.move("f0", 1.5);
    deck.touch("f0", false);
    const input = (kind: string, control: string, value: number) => ({
      method: "surface.input",
      params: { surface: "foh", control, kind, value, ts: 100 },
    });
    expect(sockets[0]!.sent.slice(1)).toEqual([
      input("turn", "e0", -2),
      input("touch", "f0", 1),
      input("value", "f0", 0.4),
      input("value", "f0", 1),
      input("touch", "f0", 0),
    ]);
    expect(state.touched).toEqual({});
  });

  it("moves a motor fader to its level unless a hand is on it", () => {
    const { deck, state, sockets } = setup({ faders: 1 });
    deck.start();
    sockets[0]!.open();
    sockets[0]!.receive({ method: "surface.level", params: { surface: "foh", control: "f0", value: 0.7 } });
    expect(state.levels.f0).toBe(0.7);
    expect(faderPosition(state, "f0", true)).toBe(0.7);
    expect(faderPosition(state, "f0", false)).toBe(0);
    deck.touch("f0", true);
    expect(faderPosition(state, "f0", true)).toBe(0.7);
    deck.move("f0", 0.2);
    sockets[0]!.receive({ method: "surface.level", params: { surface: "foh", control: "f0", value: 0.9 } });
    expect(faderPosition(state, "f0", true)).toBe(0.2);
    deck.touch("f0", false);
    expect(faderPosition(state, "f0", true)).toBe(0.9);
  });

  it("detaches on stop by closing the socket", () => {
    const { deck, state, sockets } = setup();
    deck.start();
    sockets[0]!.open();
    deck.stop();
    expect(sockets[0]!.readyState).toBe(3);
    expect(state.status).toBe("closed");
  });
});
