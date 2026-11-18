import { assert, assertEquals } from "@std/assert";
import { presets, Surfaces } from "./surfaces.ts";

class Sink {
  sent: [string, Record<string, unknown>][] = [];
  notify(m: string, p: unknown) {
    this.sent.push([m, p as Record<string, unknown>]);
  }
}

Deno.test("presets build capability lists", () => {
  const keys = presets.keys(5, 3, {
    w: 72,
    h: 72,
    format: "jpeg",
    rotation: 180,
  });
  assertEquals(keys.length, 15);
  assertEquals(keys[7], {
    id: "k7",
    at: { x: 2, y: 1 },
    inputs: [{ kind: "momentary" }],
    outputs: [{ kind: "image", w: 72, h: 72, format: "jpeg", rotation: 180 }],
  });
  assertEquals(
    presets.encoder({ id: "d0", at: { x: 0, y: 0 }, push: true }).inputs,
    [{ kind: "relative" }, { kind: "momentary" }],
  );
  assertEquals(
    presets.fader({ id: "f0", at: { x: 0, y: 0 }, motor: true }),
    {
      id: "f0",
      at: { x: 0, y: 0 },
      inputs: [{ kind: "absolute" }],
      outputs: [{ kind: "level", motor: true }],
    },
  );
  assertEquals(
    presets.row([{ id: "a", inputs: [{ kind: "touch" }] }, { id: "b" }])
      .map((c) => c.at),
    [{ x: 0, y: 0 }, { x: 1, y: 0 }],
  );
});

Deno.test("a surface attaches, reports input, takes draws and detaches", () => {
  const sink = new Sink();
  const surfaces = new Surfaces(sink);
  const deck = surfaces.attach({
    model: "elgato.mk2",
    serial: "AL12",
    name: "Deck",
    controls: presets.keys(1, 1),
  });
  assertEquals(deck.key, "elgato.mk2:AL12");
  assertEquals(sink.sent[0][0], "surface.attach");
  assertEquals(sink.sent[0][1].serial, "AL12");

  deck.down("k0", undefined, 1000);
  deck.up("k0", 1050);
  deck.turn("k0", -2, 1100);
  assertEquals(sink.sent.slice(1).map(([m, p]) => [m, p]), [
    ["surface.input", {
      surface: "elgato.mk2:AL12",
      control: "k0",
      kind: "down",
      ts: 1000,
    }],
    ["surface.input", {
      surface: "elgato.mk2:AL12",
      control: "k0",
      kind: "up",
      ts: 1050,
    }],
    ["surface.input", {
      surface: "elgato.mk2:AL12",
      control: "k0",
      kind: "turn",
      value: -2,
      ts: 1100,
    }],
  ]);
  deck.down("k0");
  assert(typeof sink.sent.at(-1)![1].ts === "number");

  const draws: number[][] = [];
  deck.onDraw((d) => draws.push([...d.image]));
  assert(surfaces.receive("surface.draw", {
    surface: "elgato.mk2:AL12",
    control: "k0",
    image: [0xff, 0xd8],
    format: "jpeg",
  }));
  assertEquals(draws, [[0xff, 0xd8]]);

  const levels: [string, number][] = [];
  deck.onLevel((l) => levels.push([l.control, l.value]));
  assert(surfaces.receive("surface.level", {
    surface: "elgato.mk2:AL12",
    control: "k0",
    value: 0.7,
  }));
  assertEquals(levels, [["k0", 0.7]]);

  deck.echo("k0", 0.5, 12);
  assertEquals(sink.sent.at(-1), ["surface.input", {
    surface: "elgato.mk2:AL12",
    control: "k0",
    kind: "value",
    value: 0.5,
    ts: 12,
    echo: true,
  }]);
  assert(!surfaces.receive("io.data", {}));

  surfaces.detachAll();
  assertEquals(sink.sent.at(-1), ["surface.detach", {
    surface: "elgato.mk2:AL12",
  }]);
  assert(!deck.attached);
  const count = sink.sent.length;
  deck.down("k0");
  assertEquals(sink.sent.length, count, "nothing after a detach");
});

Deno.test("attaching a key again replaces it", () => {
  const sink = new Sink();
  const surfaces = new Surfaces(sink);
  const first = surfaces.attach({ model: "m", name: "M", controls: [] });
  const second = surfaces.attach({ model: "m", name: "M", controls: [] });
  assert(!first.attached && second.attached);
  assertEquals(sink.sent.map(([m]) => m), [
    "surface.attach",
    "surface.detach",
    "surface.attach",
  ]);
});
