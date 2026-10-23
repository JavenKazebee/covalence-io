// The browser's virtual deck as a surface (surfaces spec §4): it attaches
// over `GET /surface/{id}`, sends key input with the browser's timestamps,
// and shows the key images the core draws.

import type { Socket } from "../live/client";
import { controlsOf, type Model } from "./model";

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
  return { status: "connecting", reason: null, retrying: false, images: {}, down: {} };
}

/**
 * `surface.attach` for a deck of `model` (plugin protocol §8): one key per
 * cell, each `momentary` with an `image`, placed with `at`.
 */
export function attachMessage(slug: string, name: string, modelId: string, model: Model) {
  const { cols } = model.layout;
  return {
    method: "surface.attach",
    params: {
      surface: slug,
      model: modelId,
      name,
      controls: controlsOf(model.layout).map((id, i) => ({
        id,
        at: { x: i % cols, y: Math.floor(i / cols) },
        inputs: [{ kind: "momentary" }],
        outputs: [{ kind: "image", w: model.key, h: model.key, format: "png" }],
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
    private readonly model: { id: string; model: Model },
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

  private open() {
    this.state.status = "connecting";
    this.state.retrying = false;
    const socket = this.connectTo(this.url);
    this.socket = socket;
    socket.onopen = () => {
      if (socket !== this.socket) return;
      this.attempt = 0;
      this.state.reason = null;
      socket.send(JSON.stringify(attachMessage(this.surface.slug, this.surface.name, this.model.id, this.model.model)));
      this.state.status = "attached";
    };
    socket.onmessage = (ev) => {
      if (socket !== this.socket) return;
      const message = JSON.parse(String(ev.data)) as { method?: string; params?: { control?: string; image?: string } };
      const { control, image } = message.params ?? {};
      if (message.method === "surface.draw" && control !== undefined && image !== undefined) {
        this.state.images[control] = `data:image/png;base64,${image}`;
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
  }

  private send(message: unknown): boolean {
    if (this.socket === null || this.state.status !== "attached" || this.socket.readyState !== 1) return false;
    this.socket.send(JSON.stringify(message));
    return true;
  }
}
