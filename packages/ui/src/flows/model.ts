// Flows: the implemented subset of the flow spec (docs/specs/flows.md §8, §9).
// Mirrors `covalence_core::flow` by hand; the shared fixtures in
// crates/covalence-core/fixtures/flows/ catch drift (ADR 0020).

import type { JsonSchema, Manifest } from "../api";
import { fieldsOf, parseValues, type Field } from "../forms/schema";

export type Concurrency = "parallel" | "queue" | "restart" | "ignore";
export const concurrencies: readonly Concurrency[] = ["parallel", "queue", "restart", "ignore"];

/** JavaScript expression source (flow spec §8.7). */
export type Expr = string;

export type Edge = "rising" | "falling" | "change";
export const edges: readonly Edge[] = ["rising", "falling", "change"];

export type Trigger =
  | { kind: "api" }
  | { kind: "event"; source: { conn: string }; event: string; filter?: Expr }
  | { kind: "state"; when: Expr; edge: Edge };

/** A value that is an expression: `{expr: "..."}` (flow spec §8.3). */
export interface ExprValue {
  expr: Expr;
}

export function isExprValue(value: unknown): value is ExprValue {
  if (typeof value !== "object" || value === null) return false;
  const keys = Object.keys(value);
  return keys.length === 1 && keys[0] === "expr" && typeof (value as ExprValue).expr === "string";
}

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
  /** Literal values, or `{expr}` at the top level. */
  params: Record<string, unknown>;
}

export interface WaitStep extends StepCommon {
  kind: "wait";
  for: string | ExprValue;
}

export interface WaitUntilStep extends StepCommon {
  kind: "waitUntil";
  until: Expr;
}

export interface SetStep extends StepCommon {
  kind: "set";
  /** `local.<name>` or `vars.<name>`. */
  path: string;
  value: Expr;
}

export interface Arm {
  when: Expr;
  steps: Step[];
}

export interface BranchStep extends StepCommon {
  kind: "branch";
  arms: Arm[];
  else?: Step[];
}

export type Step = ActionStep | WaitStep | WaitUntilStep | SetStep | BranchStep;
export type StepKind = Step["kind"];

export interface Flow {
  /** Empty in a create request. */
  id: string;
  name: string;
  /** Derived from `name` by the server when empty. */
  slug: string;
  enabled: boolean;
  triggers: Trigger[];
  /** All must be truthy when a trigger matches. */
  conditions?: Expr[];
  concurrency: Concurrency;
  failover?: "abort";
  body: Step[];
  /** View-only metadata, stored as-is. */
  editor?: Record<string, unknown>;
}

export type PathSegment = string | number;

/** A position in an expression's source, 1-based. */
export interface At {
  line: number;
  column: number;
}

/** A validation error or warning (flow spec §8.4). `at` is within an expression. */
export interface Problem {
  path: PathSegment[];
  code: string;
  message: string;
  at?: At;
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

/** `POST /expressions/evaluate` (flow spec §9): `value` is absent for `undefined`. */
export interface Evaluated {
  value?: unknown;
  error?: { code: string; message: string; at?: At };
  reads: string[];
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
 * A step as the editor holds it, with every kind's fields so switching
 * between them is cheap. Action params stay as input text until save, like
 * every other schema form; `params` keeps the stored values for when the
 * plugin hasn't loaded and there's no schema to edit them with.
 */
export interface StepDraft {
  /** Stable `v-for` key; never sent. */
  key: number;
  kind: StepKind;
  id: string;
  name: string;
  enabled: boolean;
  timeout: string;
  onError: "fail" | "continue";
  conn: string;
  action: string;
  values: Record<string, string | boolean>;
  params: Record<string, unknown>;
  /** Params given as expressions, by param key. */
  exprs: Record<string, Expr>;
  /** A literal `wait` duration. */
  for: string;
  /** A `wait` duration as an expression, or null for a literal one. */
  forExpr: Expr | null;
  until: Expr;
  path: string;
  value: Expr;
  arms: ArmDraft[];
  /** A branch's `else`, or null without one. */
  otherwise: StepDraft[] | null;
  /** Optional fields the stored step spelled out at their defaults, kept so a save changes nothing it shouldn't. */
  explicit: { enabled?: true; onError?: "fail" };
}

export interface ArmDraft {
  key: number;
  when: Expr;
  steps: StepDraft[];
}

export interface FlowDraft {
  id: string;
  name: string;
  slug: string;
  enabled: boolean;
  triggers: Trigger[];
  conditions: Expr[];
  concurrency: Concurrency;
  body: StepDraft[];
  editor?: Record<string, unknown>;
}

let nextKey = 1;

/** A deep copy of plain JSON, which flows are. Unlike `structuredClone`, it takes Vue's reactive proxies. */
const copyJson = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

export function newFlowDraft(): FlowDraft {
  return {
    id: "",
    name: "",
    slug: "",
    enabled: true,
    triggers: [{ kind: "api" }],
    conditions: [],
    concurrency: "ignore",
    body: [],
  };
}

export function newStepDraft(kind: StepKind, id: string): StepDraft {
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
    exprs: {},
    for: kind === "wait" ? "1s" : "",
    forExpr: null,
    until: "",
    path: kind === "set" ? "local." : "",
    value: "",
    arms: kind === "branch" ? [newArmDraft()] : [],
    otherwise: null,
    explicit: {},
  };
}

export function newArmDraft(): ArmDraft {
  return { key: nextKey++, when: "", steps: [] };
}

/** What every step editor in a flow shares. */
export interface StepContext {
  /** The whole body, for which steps finish before which. */
  body: StepDraft[];
  /** Every step ID in the flow. */
  all: ReadonlySet<string>;
  /** The active run's position, if one is running. */
  position: readonly string[];
}

/** Every step in a list and the branches inside it, in run order. */
export function allSteps<S extends { arms: { steps: S[] }[]; otherwise: S[] | null }>(steps: readonly S[]): S[] {
  return steps.flatMap((s) => [s, ...allSteps(s.arms.flatMap((a) => a.steps)), ...allSteps(s.otherwise ?? [])]);
}

/** The form fields for an action's params, or null when its manifest isn't known. */
export type ParamsSchema = (conn: string, action: string) => JsonSchema | null;

export function paramsSchemaFrom(manifestOf: (conn: string) => Manifest | null): ParamsSchema {
  return (conn, action) => manifestOf(conn)?.actions.find((a) => a.id === action)?.params ?? null;
}

/** Stored params as input text: what `initialValues` gives for a fresh form. Expressions are left out. */
export function valuesOf(fields: Field[], params: Record<string, unknown>): Record<string, string | boolean> {
  const out: Record<string, string | boolean> = {};
  for (const field of fields) {
    const v = isExprValue(params[field.key]) ? undefined : params[field.key];
    if (field.kind === "boolean") out[field.key] = v === true;
    else out[field.key] = v === undefined || v === null ? "" : String(v);
  }
  return out;
}

function stepToDraft(step: Step, schemaOf: ParamsSchema): StepDraft {
  const draft = newStepDraft(step.kind, step.id);
  draft.name = step.name ?? "";
  draft.enabled = step.enabled !== false;
  draft.timeout = step.timeout ?? "";
  draft.onError = step.onError ?? "fail";
  if (step.enabled === true) draft.explicit.enabled = true;
  if (step.onError === "fail") draft.explicit.onError = "fail";
  switch (step.kind) {
    case "wait":
      if (isExprValue(step.for)) {
        draft.forExpr = step.for.expr;
        draft.for = "";
      } else {
        draft.for = step.for;
      }
      break;
    case "waitUntil":
      draft.until = step.until;
      break;
    case "set":
      draft.path = step.path;
      draft.value = step.value;
      break;
    case "branch":
      draft.arms = step.arms.map((arm) => ({
        key: nextKey++,
        when: arm.when,
        steps: arm.steps.map((s) => stepToDraft(s, schemaOf)),
      }));
      draft.otherwise = step.else === undefined ? null : step.else.map((s) => stepToDraft(s, schemaOf));
      break;
    case "action": {
      draft.conn = step.target.conn;
      draft.action = step.action;
      draft.params = copyJson(step.params);
      for (const [k, v] of Object.entries(step.params)) if (isExprValue(v)) draft.exprs[k] = v.expr;
      const schema = schemaOf(step.target.conn, step.action);
      if (schema !== null) draft.values = valuesOf(fieldsOf(schema), step.params);
      break;
    }
  }
  return draft;
}

export function toDraft(flow: Flow, schemaOf: ParamsSchema): FlowDraft {
  return {
    id: flow.id,
    name: flow.name,
    slug: flow.slug,
    enabled: flow.enabled,
    triggers: copyJson(flow.triggers),
    conditions: [...(flow.conditions ?? [])],
    concurrency: flow.concurrency,
    editor: flow.editor,
    body: flow.body.map((step) => stepToDraft(step, schemaOf)),
  };
}

export type DraftResult =
  | { ok: true; flow: Flow }
  /** Param errors by step key, then field key. */
  | { ok: false; errors: Map<number, Record<string, string>> };

function stepFromDraft(d: StepDraft, schemaOf: ParamsSchema, errors: Map<number, Record<string, string>>): Step {
  const common: StepCommon = { id: d.id.trim() };
  if (d.name.trim() !== "") common.name = d.name.trim();
  if (!d.enabled) common.enabled = false;
  else if (d.explicit.enabled) common.enabled = true;
  if (d.timeout.trim() !== "") common.timeout = d.timeout.trim();
  if (d.onError === "continue" || d.explicit.onError === "fail") common.onError = d.onError;
  const steps = (list: StepDraft[]) => list.map((s) => stepFromDraft(s, schemaOf, errors));
  switch (d.kind) {
    case "wait":
      return { ...common, kind: "wait", for: d.forExpr === null ? d.for.trim() : { expr: d.forExpr } };
    case "waitUntil":
      return { ...common, kind: "waitUntil", until: d.until };
    case "set":
      return { ...common, kind: "set", path: d.path.trim(), value: d.value };
    case "branch": {
      const step: BranchStep = {
        ...common,
        kind: "branch",
        arms: d.arms.map((arm) => ({ when: arm.when, steps: steps(arm.steps) })),
      };
      if (d.otherwise !== null) step.else = steps(d.otherwise);
      return step;
    }
    case "action": {
      let params = d.params;
      const schema = schemaOf(d.conn, d.action);
      if (schema !== null) {
        // Expression params are the server's to check, when the step runs.
        const literal = fieldsOf(schema).filter((f) => !(f.key in d.exprs));
        const parsed = parseValues(literal, d.values);
        if (parsed.ok) params = parsed.value;
        else errors.set(d.key, parsed.errors);
      } else {
        params = Object.fromEntries(Object.entries(params).filter(([k]) => !(k in d.exprs)));
      }
      const exprs = Object.fromEntries(Object.entries(d.exprs).map(([k, expr]) => [k, { expr }]));
      return { ...common, kind: "action", target: { conn: d.conn }, action: d.action, params: { ...params, ...exprs } };
    }
  }
}

/**
 * The flow to send. Literal params are parsed against their schema here, so
 * a typo shows next to its input; everything else is the server's to
 * validate. Optional fields left empty are left out, as the fixtures write them.
 */
export function fromDraft(draft: FlowDraft, schemaOf: ParamsSchema): DraftResult {
  const errors = new Map<number, Record<string, string>>();
  const body = draft.body.map((d) => stepFromDraft(d, schemaOf, errors));
  if (errors.size > 0) return { ok: false, errors };

  // An empty filter is no filter.
  const triggers = draft.triggers.map((t): Trigger => {
    if (t.kind !== "event" || t.filter === undefined || t.filter.trim() !== "") return t;
    return { kind: "event", source: t.source, event: t.event };
  });
  const flow: Flow = {
    id: draft.id,
    name: draft.name,
    slug: draft.slug.trim(),
    enabled: draft.enabled,
    triggers,
    concurrency: draft.concurrency,
    failover: "abort",
    body,
  };
  if (draft.conditions.length > 0) flow.conditions = draft.conditions;
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

/** A step's param problems keyed by param, for `SchemaForm`'s `errors`. `step` is its path, or its index in the body. */
export function paramProblems(problems: readonly ShownProblem[], step: readonly PathSegment[] | number): Record<string, string> {
  const at = typeof step === "number" ? ["body", step] : step;
  const out: Record<string, string> = {};
  for (const p of problemsAt(problems, [...at, "params"])) {
    const key = p.path[at.length + 1];
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
  switch (trigger.kind) {
    case "api":
      return "API / Run button";
    case "event":
      return `${alias(trigger.source.conn)} ${trigger.event}${trigger.filter ? " (filtered)" : ""}`;
    case "state":
      return `${trigger.when} (${trigger.edge})`;
  }
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
