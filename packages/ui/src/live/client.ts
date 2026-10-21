import type { LiveStore } from "../stores/live";
import { isUnderAny } from "./paths";
import type { ClientMessage, JournalEntry, ServerMessage } from "./protocol";

/** The parts of `WebSocket` the client uses, so tests can pass a fake. */
export interface Socket {
  readonly readyState: number;
  onopen: ((ev: Event) => unknown) | null;
  onclose: ((ev: CloseEvent) => unknown) | null;
  onmessage: ((ev: MessageEvent) => unknown) | null;
  send(data: string): void;
  close(): void;
}

export interface LiveClientOptions {
  /** Defaults to `/ws` on the page's own host, which Vite proxies in dev. */
  url?: string;
  connect?: (url: string) => Socket;
  /** First reconnect delay; it doubles up to `maxBackoffMs`. */
  minBackoffMs?: number;
  maxBackoffMs?: number;
}

export interface JournalListener {
  entry(entry: JournalEntry): void;
  /** The tail dropped `missed` entries: refetch them over `GET /journal`. */
  lagged?(missed: number): void;
}

const OPEN = 1;

/**
 * The one WebSocket to `/ws`. Path subscriptions and the journal tail are
 * reference-counted across components; after a reconnect everything still
 * held is subscribed again, and the snapshots that answer replace what the
 * store had.
 */
export class LiveClient {
  private socket: Socket | null = null;
  private readonly refs = new Map<string, number>();
  private readonly journal = new Set<JournalListener>();
  private attempt = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private stopped = true;
  private readonly url: string;
  private readonly connectTo: (url: string) => Socket;
  private readonly minBackoffMs: number;
  private readonly maxBackoffMs: number;

  constructor(
    private readonly store: LiveStore,
    options: LiveClientOptions = {},
  ) {
    this.url = options.url ?? defaultUrl();
    this.connectTo = options.connect ?? ((url) => new WebSocket(url));
    this.minBackoffMs = options.minBackoffMs ?? 250;
    this.maxBackoffMs = options.maxBackoffMs ?? 10_000;
  }

  /** Opens the socket, and keeps reopening it until `stop`. */
  start() {
    if (!this.stopped) return;
    this.stopped = false;
    this.open();
  }

  stop() {
    this.stopped = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    const socket = this.socket;
    this.socket = null;
    socket?.close();
    this.store.connected = false;
  }

  /** Paths currently subscribed, each once. */
  get paths(): string[] {
    return [...this.refs.keys()];
  }

  subscribe(path: string) {
    const count = this.refs.get(path) ?? 0;
    this.refs.set(path, count + 1);
    if (count === 0) this.send({ op: "subscribe", paths: [path] });
  }

  unsubscribe(path: string) {
    const count = this.refs.get(path);
    if (count === undefined) return;
    if (count > 1) {
      this.refs.set(path, count - 1);
      return;
    }
    this.refs.delete(path);
    this.send({ op: "unsubscribe", paths: [path] });
    this.store.prune([path], this.refs.keys());
  }

  /** Follows the live journal tail until the returned function is called. */
  followJournal(listener: JournalListener): () => void {
    this.journal.add(listener);
    if (this.journal.size === 1) this.send({ op: "journal", on: true });
    return () => {
      if (!this.journal.delete(listener)) return;
      if (this.journal.size === 0) this.send({ op: "journal", on: false });
    };
  }

  private open() {
    const socket = this.connectTo(this.url);
    this.socket = socket;
    socket.onopen = () => {
      if (socket !== this.socket) return;
      this.attempt = 0;
      this.store.connected = true;
      this.store.lastError = null;
      if (this.refs.size > 0) this.send({ op: "subscribe", paths: this.paths });
      if (this.journal.size > 0) this.send({ op: "journal", on: true });
    };
    socket.onmessage = (ev) => {
      if (socket !== this.socket) return;
      this.receive(JSON.parse(String(ev.data)) as ServerMessage);
    };
    socket.onclose = () => {
      if (socket !== this.socket) return;
      this.socket = null;
      this.store.connected = false;
      // Unknown until the server says again.
      this.store.host = null;
      if (!this.stopped) this.retry();
    };
  }

  private retry() {
    const base = Math.min(this.maxBackoffMs, this.minBackoffMs * 2 ** this.attempt);
    this.attempt += 1;
    // Up to 25% jitter, so many tabs don't reconnect in step.
    const delay = base * (0.75 + Math.random() * 0.25);
    this.timer = setTimeout(() => {
      this.timer = null;
      if (!this.stopped) this.open();
    }, delay);
  }

  private send(message: ClientMessage) {
    // While closed there's nothing to do: `onopen` sends the whole picture.
    if (this.socket?.readyState === OPEN) this.socket.send(JSON.stringify(message));
  }

  /** Applies one server message. Public for tests. */
  receive(message: ServerMessage) {
    switch (message.type) {
      case "host":
        this.store.host = message.host;
        break;
      case "snapshot": {
        // A path unsubscribed while its snapshot was in flight is not wanted.
        const paths = message.paths.filter((path) => this.refs.has(path));
        const leaves = message.leaves.filter((leaf) => isUnderAny(leaf.path, paths));
        this.store.applySnapshot(paths, leaves);
        break;
      }
      case "patch": {
        const set = message.set.filter((leaf) => isUnderAny(leaf.path, this.refs.keys()));
        this.store.applyPatch(set, message.deleted);
        break;
      }
      case "journal":
        if ("entry" in message) {
          for (const listener of this.journal) listener.entry(message.entry);
        }
        break;
      case "lagged":
        for (const listener of this.journal) listener.lagged?.(message.missed);
        break;
      case "error":
        this.store.lastError = message.message;
        console.warn(`/ws: ${message.message}`);
        break;
      case "unsubscribed":
        break;
    }
  }
}

function defaultUrl(): string {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}/ws`;
}
