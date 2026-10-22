// Messages on `GET /ws`: the state spec, §1.6.

export type Quality = "good" | "stale" | "unknown";

/** One state leaf, shaped as in `GET /state`. */
export interface Leaf {
  /** Text path (§1.1), e.g. `vh.outputs[0].source`. */
  path: string;
  value: unknown;
  quality: Quality;
  /** Wall-clock time of the last change. */
  ts: number;
  /** Journal sequence number of the patch that set it. */
  seq: number;
}

export interface HostStatus {
  phase: "starting" | "ready" | "down" | "stopped";
  /** Counts host processes started, from 1. */
  generation: number;
  pid: number | null;
}

/** Who produced a journal entry (§2.1). */
export type Origin =
  | { user: string; client: string }
  | { conn: string }
  | { surface: string; orbital?: string }
  /** `step` only on step-level entries (flow spec §8.5). */
  | { flow: string; run: string; step?: string }
  | { core: string };

/** A runtime journal entry (§2.1). `data` depends on `kind` (§2.2). */
export interface JournalEntry {
  seq: number;
  term: number;
  /** RFC 3339, for display only. */
  ts: string;
  mono: number;
  origin: Origin;
  /** Seq of the entry that directly caused this one. */
  cause?: number;
  run?: string;
  kind: string;
  data: unknown;
}

export type ClientMessage =
  | { op: "subscribe"; paths: string[] }
  | { op: "unsubscribe"; paths: string[] }
  | { op: "journal"; on: boolean };

export type ServerMessage =
  | { type: "host"; host: HostStatus }
  | { type: "snapshot"; paths: string[]; leaves: Leaf[] }
  | { type: "patch"; set: Leaf[]; deleted: string[] }
  | { type: "unsubscribed"; paths: string[] }
  | { type: "journal"; entry: JournalEntry }
  | { type: "journal"; on: boolean }
  | { type: "lagged"; feed: "journal"; missed: number }
  | { type: "error"; message: string };
