// The browser's virtual deck as a surface (surfaces spec §4): it attaches
// over `GET /surface/{id}`, sends input with the browser's timestamps, and
// shows the key images and levels the core sets. Besides its keys it can
// have encoders and faders (surfaces spec §11.4).

import type { Socket } from "../live/client";
import { deckControls, type Model, type VirtualLayout } from "./model";

export interface DeckState {
  status: "connecting" | "attached" | "closed";
  /** Why the socket closed, when the server said, or `null`. */
  reason: string | null;
  /** Whether it will reconnect by itself. */
  retrying: boolean;
  /** `data:` URLs of the latest image per control. */
  images: Record<string, string>;
  /** Controls held down right now. */
  down: Record<string, boolean>;
  /** The level the core last set on each `level` output, 0 to 1. */
  levels: Record<string, number>;
  /** Faders touched right now. */
  touched: Record<string, boolean>;
  /** Where each fader was last moved to by hand, 0 to 1. */
  positions: Record<string, number>;
}

export interface DeckOptions {
  url?: string;
  connect?: (url: string) => Socket;
  /** The browser's monotonic clock, in milliseconds. */
  now?: () => number;
  minBackoffMs?: number;
  maxBackoffMs?: number;
}

export function newDeckState(): DeckState {
  return { status: "connecting", reason: null, retrying: false, images: {}, down: {}, levels: {}, touched: {}, positions: {} };
}

/**
 * Where a fader shows: under the hand while it's touched, else at its level
 * if it has a motor, else where it was left.
 */
export function faderPosition(state: DeckState, control: string, motor: boolean): number {
  const hand = state.positions[control];
  if (state.touched[control] || !motor) return hand ?? 0;
  return state.levels[control] ?? hand ?? 0;
}

/**
 * `surface.attach` for a deck of `model` (plugin protocol §8): one key per
 * cell, each `momentary` with an `image`, placed with `at`, and `layout`'s
 * encoders and faders.
 */
export function attachMessage(slug: string, name: string, modelId: string, model: Model, layout?: VirtualLayout) {
  return {
    method: "surface.attach",
    params: {
      surface: slug,
      model: modelId,
      name,
      controls: deckControls(model, layout).map((c) => ({
        ...c,
        outputs: (c.outputs ?? []).map((o) => (o.kind === "image" ? { kind: "image", w: model.key, h: model.key, format: "png" } : o)),
      })),
    },
  };
}

/**
 * Closes that mean "stop": the surface was attached in another tab or
 * deleted (1000), or the server refused what was sent (1008). Reconnecting
 * after either would only fight the other tab or fail again.
 */
const FINAL = new Set([1000, 1008]);

export class DeckClient {
  private socket: Socket | null = null;
  private stopped = true;
  private attempt = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private readonly url: string;
  private readonly connectTo: (url: string) => Socket;
  private readonly now: () => number;
  private readonly minBackoffMs: number;
  private readonly maxBackoffMs: number;

  constructor(
    private readonly state: DeckState,
    private readonly surface: { id: string; slug: string; name: string },
    private readonly model: { id: string; model: Model; layout?: VirtualLayout },
    options: DeckOptions = {},
  ) {
    const scheme = typeof location !== "undefined" && location.protocol === "https:" ? "wss:" : "ws:";
    this.url = options.url ?? `${scheme}//${location.host}/surface/${encodeURIComponent(surface.id)}`;
    this.connectTo = options.connect ?? ((url) => new WebSocket(url));
    this.now = options.now ?? (() => performance.now());
    this.minBackoffMs = options.minBackoffMs ?? 500;
    this.maxBackoffMs = options.maxBackoffMs ?? 10_000;
  }

  /** Attaches, and reattaches after the connection drops, until `stop`. */
  start() {
    if (!this.stopped) return;
    this.stopped = false;
    this.attempt = 0;
    this.open();
  }

  /** Detaches: the socket closing is the detach. */
  stop() {
    this.stopped = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    const socket = this.socket;
    this.socket = null;
    socket?.close();
    this.released();
    this.state.status = "closed";
    this.state.retrying = false;
  }

  down(control: string) {
    if (this.state.down[control]) return;
    if (this.send({ method: "surface.input", params: { surface: this.surface.slug, control, kind: "down", ts: this.now() } })) {
      this.state.down[control] = true;
    }
  }

  up(control: string) {
    if (!this.state.down[control]) return;
    delete this.state.down[control];
    this.send({ method: "surface.input", params: { surface: this.surface.slug, control, kind: "up", ts: this.now() } });
  }

  /** An encoder turned by whole detents, clockwise positive. */
  turn(control: string, detents: number) {
    if (detents === 0 || !Number.isInteger(detents)) return;
    this.input(control, "turn", detents);
  }

  /** A fader touched or let go. It reports its position only while touched. */
  touch(control: string, touched: boolean) {
    if (Boolean(this.state.touched[control]) === touched) return;
    if (!touched) {
      delete this.state.touched[control];
      this.input(control, "touch", 0);
      return;
    }
    if (this.input(control, "touch", 1)) {
      this.state.touched[control] = true;
      // It starts from where it shows.
      if (this.state.positions[control] === undefined && this.state.levels[control] !== undefined) {
        this.state.positions[control] = this.state.levels[control];
      }
    }
  }

  /** A touched fader moved to `value`, 0 to 1. */
  move(control: string, value: number) {
    if (!this.state.touched[control]) return;
    const v = Math.min(1, Math.max(0, value));
    if (this.state.positions[control] === v) return;
    this.state.positions[control] = v;
    this.input(control, "value", v);
  }

  private open() {
    this.state.status = "connecting";
    this.state.retrying = false;
    const socket = this.connectTo(this.url);
    this.socket = socket;
    socket.onopen = () => {
      if (socket !== this.socket) return;
      this.attempt = 0;
      this.state.reason = null;
      socket.send(
        JSON.stringify(attachMessage(this.surface.slug, this.surface.name, this.model.id, this.model.model, this.model.layout)),
      );
      this.state.status = "attached";
    };
    socket.onmessage = (ev) => {
      if (socket !== this.socket) return;
      const message = JSON.parse(String(ev.data)) as { method?: string; params?: { control?: string; image?: string; value?: number } };
      const { control, image, value } = message.params ?? {};
      if (message.method === "surface.draw" && control !== undefined && image !== undefined) {
        this.state.images[control] = `data:image/png;base64,${image}`;
      } else if (message.method === "surface.level" && control !== undefined && typeof value === "number") {
        this.state.levels[control] = value;
      }
    };
    socket.onclose = (ev) => {
      if (socket !== this.socket) return;
      this.socket = null;
      // The core released every held key when the surface detached.
      this.released();
      this.state.status = "closed";
      this.state.reason = ev.reason || null;
      if (this.stopped || FINAL.has(ev.code)) return;
      this.retry();
    };
  }

  private retry() {
    const delay = Math.min(this.maxBackoffMs, this.minBackoffMs * 2 ** this.attempt);
    this.attempt += 1;
    this.state.retrying = true;
    this.timer = setTimeout(() => {
      this.timer = null;
      if (!this.stopped) this.open();
    }, delay);
  }

  private released() {
    for (const control of Object.keys(this.state.down)) delete this.state.down[control];
    for (const control of Object.keys(this.state.touched)) delete this.state.touched[control];
  }

  private input(control: string, kind: string, value: number): boolean {
    return this.send({ method: "surface.input", params: { surface: this.surface.slug, control, kind, value, ts: this.now() } });
  }

  private send(message: unknown): boolean {
    if (this.socket === null || this.state.status !== "attached" || this.socket.readyState !== 1) return false;
    this.socket.send(JSON.stringify(message));
    return true;
  }
}
