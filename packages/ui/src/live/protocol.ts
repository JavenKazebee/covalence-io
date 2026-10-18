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

/** A runtime journal entry (§2.1). Phase 4 gives it a closer type. */
export interface JournalEntry {
  seq: number;
  ts: string;
  kind: string;
  cause?: number;
  [field: string]: unknown;
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
