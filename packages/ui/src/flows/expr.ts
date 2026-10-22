// Editor checks on expressions (flow spec §8.7). The server compiles them and
// reports syntax errors; these are the warnings only the editor gives.

import { parser } from "@lezer/javascript";
import type { StepDraft } from "./model";

/** A `steps.<id>` reference in an expression's source, with its place. */
export interface StepRef {
  id: string;
  from: number;
  to: number;
}

/**
 * Every `steps.<id>` (and `steps?.<id>`) in `source`, found with the
 * JavaScript syntax tree, so text in strings and comments doesn't count.
 * The source is parsed as a function body, like the server's fallback.
 */
export function stepRefs(source: string): StepRef[] {
  const tree = parser.configure({ top: "Script" }).parse(source);
  const out: StepRef[] = [];
  tree.iterate({
    enter(node) {
      if (node.name !== "MemberExpression") return;
      const object = node.node.firstChild;
      if (object?.name !== "VariableName" || source.slice(object.from, object.to) !== "steps") return;
      const property = node.node.getChild("PropertyName");
      if (property === null) return;
      out.push({ id: source.slice(property.from, property.to), from: property.from, to: property.to });
    },
  });
  return out;
}

/**
 * The step IDs an expression in `step` can read: the steps that finish
 * before it runs. Earlier siblings and everything inside them count; the
 * branches it sits in don't, since they finish after it. `step` null is the
 * flow's triggers and conditions, where no step has run.
 */
export function stepsBefore(body: readonly StepDraft[], step: StepDraft | null): Set<string> {
  const known = new Set<string>();
  const add = (s: StepDraft) => {
    known.add(s.id);
    for (const arm of s.arms) arm.steps.forEach(add);
    s.otherwise?.forEach(add);
  };
  // Returns true once `step` is found, having added what comes before it.
  const walk = (list: readonly StepDraft[]): boolean => {
    for (const s of list) {
      if (s === step) return true;
      if (s.arms.some((a) => walk(a.steps)) || walk(s.otherwise ?? [])) return true;
      add(s);
    }
    return false;
  };
  if (step !== null) walk(body);
  return known;
}

/** Warnings for the step references in `source` that `known` doesn't have. */
export function stepWarnings(source: string, known: ReadonlySet<string>, all: ReadonlySet<string>): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const ref of stepRefs(source)) {
    if (known.has(ref.id) || seen.has(ref.id)) continue;
    seen.add(ref.id);
    out.push(
      all.has(ref.id)
        ? `Step ${ref.id} hasn't finished when this runs, so steps.${ref.id} is undefined.`
        : `There's no step ${ref.id}, so steps.${ref.id} is undefined.`,
    );
  }
  return out;
}
