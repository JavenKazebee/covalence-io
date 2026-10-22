// Flows: the M3 subset of the flow spec (docs/specs/flows.md §8, §9).
// Mirrors `covalence_core::flow` by hand; the shared fixtures in
// crates/covalence-core/fixtures/flows/ catch drift (ADR 0020).

import type { JsonSchema, Manifest } from "../api";
import { fieldsOf, parseValues, type Field } from "../forms/schema";

export type Concurrency = "parallel" | "queue" | "restart" | "ignore";
export const concurrencies: readonly Concurrency[] = ["parallel", "queue", "restart", "ignore"];

export type Trigger = { kind: "api" } | { kind: "event"; source: { conn: string }; event: string };

interface StepCommon {
  /** Unique within the flow: `[a-z][A-Za-z0-9_]*`. */
  id: string;
  name?: string;
  /** Absent means enabled. */
  enabled?: boolean;
  timeout?: string;
  onError?: "fail" | "continue";
}

export interface ActionStep extends StepCommon {
  kind: "action";
  target: { conn: string };
  action: string;
  params: Record<string, unknown>;
}

export interface WaitStep extends StepCommon {
  kind: "wait";
  for: string;
}

export type Step = ActionStep | WaitStep;

export interface Flow {
  /** Empty in a create request. */
  id: string;
  name: string;
  /** Derived from `name` by the server when empty. */
  slug: string;
  enabled: boolean;
  triggers: Trigger[];
  concurrency: Concurrency;
  failover?: "abort";
  body: Step[];
  /** View-only metadata, stored as-is. */
  editor?: Record<string, unknown>;
}

export type PathSegment = string | number;

/** A validation error or warning (flow spec §8.4). */
export interface Problem {
  path: PathSegment[];
  code: string;
  message: string;
}

/** A problem as the editor shows it: errors block a save, warnings don't. */
export interface ShownProblem extends Problem {
  warning: boolean;
}

export interface Saved {
  flow: Flow;
  warnings: Problem[];
}

export type RunStatus = "queued" | "running" | "ok" | "failed" | "cancelled";

export interface RunError {
  code: string;
  message: string;
}

/** `GET /runs` (flow spec §9). Seqs are journal entries `why` can open. */
export interface RunInfo {
  run: string;
  flow: string;
  status: RunStatus;
  matched: number;
  started?: number;
  finished?: number;
  startedAt?: number;
  durationMs?: number;
  position?: string[];
  error?: RunError;
}

export type FireResult =
  | { matched: number; status: "started" | "queued"; run: string }
  | { matched: number; status: "blocked"; reason: string };

export const isActive = (status: RunStatus) => status === "queued" || status === "running";

/** Matches the server's duration check (flow spec §8.3): `500ms`, `2s`, `1m30s`. */
export function isDuration(text: string): boolean {
  const m = /^(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?(?:(\d+)ms)?$/.exec(text);
  return m !== null && text !== "" && m.slice(1).some((n) => n !== undefined && Number(n) > 0);
}

/** A step ID nothing else in `taken` has: `route1`, `route2`, … */
export function freshStepId(base: string, taken: Iterable<string>): string {
  const used = new Set(taken);
  const stem = /^[a-z][A-Za-z0-9_]*$/.test(base) ? base : "step";
  for (let n = 1; ; n++) {
    if (!used.has(`${stem}${n}`)) return `${stem}${n}`;
  }
}

// ---- Editing ----

/**
 * A step as the editor holds it. Action params stay as input text until save,
 * like every other schema form; `params` keeps the stored values for when the
 * plugin hasn't loaded and there's no schema to edit them with.
 */
export interface StepDraft {
  /** Stable `v-for` key; never sent. */
  key: number;
  kind: "action" | "wait";
  id: string;
  name: string;
  enabled: boolean;
  timeout: string;
  onError: "fail" | "continue";
  conn: string;
  action: string;
  values: Record<string, string | boolean>;
  params: Record<string, unknown>;
  for: string;
  /** Optional fields the stored step spelled out at their defaults, kept so a save changes nothing it shouldn't. */
  explicit: { enabled?: true; onError?: "fail" };
}

export interface FlowDraft {
  id: string;
  name: string;
  slug: string;
  enabled: boolean;
  triggers: Trigger[];
  concurrency: Concurrency;
  body: StepDraft[];
  editor?: Record<string, unknown>;
}

let nextKey = 1;

export function newFlowDraft(): FlowDraft {
  return { id: "", name: "", slug: "", enabled: true, triggers: [{ kind: "api" }], concurrency: "ignore", body: [] };
}

export function newStepDraft(kind: StepDraft["kind"], id: string): StepDraft {
  return {
    key: nextKey++,
    kind,
    id,
    name: "",
    enabled: true,
    timeout: "",
    onError: "fail",
    conn: "",
    action: "",
    values: {},
    params: {},
    for: kind === "wait" ? "1s" : "",
    explicit: {},
  };
}

/** The form fields for an action's params, or null when its manifest isn't known. */
export type ParamsSchema = (conn: string, action: string) => JsonSchema | null;

export function paramsSchemaFrom(manifestOf: (conn: string) => Manifest | null): ParamsSchema {
  return (conn, action) => manifestOf(conn)?.actions.find((a) => a.id === action)?.params ?? null;
}

/** Stored params as input text: what `initialValues` gives for a fresh form. */
export function valuesOf(fields: Field[], params: Record<string, unknown>): Record<string, string | boolean> {
  const out: Record<string, string | boolean> = {};
  for (const field of fields) {
    const v = params[field.key];
    if (field.kind === "boolean") out[field.key] = v === true;
    else out[field.key] = v === undefined || v === null ? "" : String(v);
  }
  return out;
}

export function toDraft(flow: Flow, schemaOf: ParamsSchema): FlowDraft {
  return {
    id: flow.id,
    name: flow.name,
    slug: flow.slug,
    enabled: flow.enabled,
    triggers: structuredClone(flow.triggers),
    concurrency: flow.concurrency,
    editor: flow.editor,
    body: flow.body.map((step) => {
      const draft = newStepDraft(step.kind, step.id);
      draft.name = step.name ?? "";
      draft.enabled = step.enabled !== false;
      draft.timeout = step.timeout ?? "";
      draft.onError = step.onError ?? "fail";
      if (step.enabled === true) draft.explicit.enabled = true;
      if (step.onError === "fail") draft.explicit.onError = "fail";
      if (step.kind === "wait") {
        draft.for = step.for;
      } else {
        draft.conn = step.target.conn;
        draft.action = step.action;
        draft.params = structuredClone(step.params);
        const schema = schemaOf(step.target.conn, step.action);
        if (schema !== null) draft.values = valuesOf(fieldsOf(schema), step.params);
      }
      return draft;
    }),
  };
}

export type DraftResult =
  | { ok: true; flow: Flow }
  /** Param errors by step key, then field key. */
  | { ok: false; errors: Map<number, Record<string, string>> };

/**
 * The flow to send. Params are parsed against their schema here, so a typo
 * shows next to its input; everything else is the server's to validate.
 * Optional fields left empty are left out, as the fixtures write them.
 */
export function fromDraft(draft: FlowDraft, schemaOf: ParamsSchema): DraftResult {
  const errors = new Map<number, Record<string, string>>();
  const body = draft.body.map((d): Step => {
    const common: StepCommon = { id: d.id.trim() };
    if (d.name.trim() !== "") common.name = d.name.trim();
    if (!d.enabled) common.enabled = false;
    else if (d.explicit.enabled) common.enabled = true;
    if (d.timeout.trim() !== "") common.timeout = d.timeout.trim();
    if (d.onError === "continue" || d.explicit.onError === "fail") common.onError = d.onError;
    if (d.kind === "wait") return { ...common, kind: "wait", for: d.for.trim() };

    let params = d.params;
    const schema = schemaOf(d.conn, d.action);
    if (schema !== null) {
      const parsed = parseValues(fieldsOf(schema), d.values);
      if (parsed.ok) params = parsed.value;
      else errors.set(d.key, parsed.errors);
    }
    return { ...common, kind: "action", target: { conn: d.conn }, action: d.action, params };
  });
  if (errors.size > 0) return { ok: false, errors };

  const flow: Flow = {
    id: draft.id,
    name: draft.name,
    slug: draft.slug.trim(),
    enabled: draft.enabled,
    triggers: draft.triggers,
    concurrency: draft.concurrency,
    failover: "abort",
    body,
  };
  if (draft.editor !== undefined) flow.editor = draft.editor;
  return { ok: true, flow };
}

// ---- Problems ----

function startsWith(path: readonly PathSegment[], prefix: readonly PathSegment[]): boolean {
  return prefix.every((segment, i) => path[i] === segment);
}

/** Problems at or below `prefix`, minus those below any of `except`. */
export function problemsAt<P extends Problem>(
  problems: readonly P[],
  prefix: readonly PathSegment[],
  except: readonly PathSegment[][] = [],
): P[] {
  return problems.filter((p) => startsWith(p.path, prefix) && !except.some((e) => startsWith(p.path, e)));
}

/** A step's param problems keyed by param, for `SchemaForm`'s `errors`. */
export function paramProblems(problems: readonly ShownProblem[], stepIndex: number): Record<string, string> {
  const out: Record<string, string> = {};
  for (const p of problemsAt(problems, ["body", stepIndex, "params"])) {
    const key = p.path[3];
    if (key !== undefined) out[String(key)] = out[String(key)] ?? p.message;
  }
  return out;
}

/** `body[1].params.input`, for a problem shown away from its field. */
export function pathText(path: readonly PathSegment[]): string {
  return path.map((s, i) => (typeof s === "number" ? `[${s}]` : i === 0 ? s : `.${s}`)).join("");
}

// ---- Status ----

/** `flow.<slug>.lastRun` (flow spec §5.4). */
export interface LastRun {
  status?: RunStatus;
  startedAt?: number;
  durationMs?: number;
  error?: RunError;
}

export function describeTrigger(trigger: Trigger, alias: (conn: string) => string): string {
  return trigger.kind === "api" ? "API / Run button" : `${alias(trigger.source.conn)} ${trigger.event}`;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
  const m = Math.floor(ms / 60_000);
  return `${m} m ${Math.round((ms % 60_000) / 1000)} s`;
}

export function runTone(status: RunStatus | undefined): "good" | "warn" | "bad" | "muted" {
  switch (status) {
    case "ok":
      return "good";
    case "failed":
      return "bad";
    case "cancelled":
    case "queued":
    case "running":
      return "warn";
    default:
      return "muted";
  }
}
