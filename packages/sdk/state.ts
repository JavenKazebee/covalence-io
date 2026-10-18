/**
 * The plugin-side mirror of a connection's state subtree, and the patch
 * coalescer.
 *
 * Patches are sent as leaf values: an object written with `set` or `patch` is
 * split into one op per leaf, so the core keeps per-leaf metadata (state spec
 * §1.3). Arrays, binary and empty objects are leaves. In an object value, keys
 * that are canonical integers (`"0"`, `"12"`) become integer segments, so
 * `{outputs: {3: {...}}}` writes `["outputs", 3, ...]`. Use a `Map` to keep a
 * key like `"3"` a string.
 *
 * Writes whose value is unchanged produce no op.
 *
 * @module
 */

import type { PatchOp, PathSegment, PathValue } from "@covalence-io/protocol";
import { isPrefix, type Path, pathKey } from "./paths.ts";

type Node = Map<PathSegment, Node> | Leaf;
type Leaf = { leaf: unknown };

const isLeaf = (n: Node | undefined): n is Leaf =>
  n !== undefined && !(n instanceof Map);

function isPlainObject(v: unknown): v is Record<string, unknown> {
  if (v === null || typeof v !== "object") return false;
  const proto = Object.getPrototypeOf(v);
  return proto === Object.prototype || proto === null;
}

function isBranch(
  v: unknown,
): v is Map<unknown, unknown> | Record<string, unknown> {
  return v instanceof Map
    ? true
    : isPlainObject(v) && Object.keys(v).length > 0;
}

const INT_KEY = /^(0|-?[1-9]\d*)$/;

function entries(v: Map<unknown, unknown> | Record<string, unknown>) {
  if (v instanceof Map) return [...v.entries()] as [PathSegment, unknown][];
  return Object.entries(v).map(([k, x]) =>
    [INT_KEY.test(k) && Number.isSafeInteger(Number(k)) ? Number(k) : k, x] as [
      PathSegment,
      unknown,
    ]
  );
}

function deepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || !a || !b) return false;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) {
      return false;
    }
    return a.every((x, i) => deepEqual(x, b[i]));
  }
  if (a instanceof Uint8Array || b instanceof Uint8Array) {
    if (!(a instanceof Uint8Array && b instanceof Uint8Array)) return false;
    return a.length === b.length && a.every((x, i) => x === b[i]);
  }
  const ka = Object.keys(a), kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every((k) =>
    deepEqual(
      (a as Record<string, unknown>)[k],
      (b as Record<string, unknown>)[k],
    )
  );
}

/** A tree of state values that turns writes into patch ops. */
export class StateTree {
  #root = new Map<PathSegment, Node>();

  /** Reads a value. Branches come back as plain objects. */
  get(path: Path): unknown {
    const node = this.#find(path);
    return node === undefined ? undefined : toValue(node);
  }

  /** Replaces the value at `path`; keys missing from `value` are deleted. */
  set(path: Path, value: unknown): PatchOp[] {
    const ops: PatchOp[] = [];
    if (path.length === 0) {
      if (!isBranch(value) && !isPlainObject(value)) {
        throw new TypeError("the state root must be an object");
      }
      this.#write(this.#root, path, value, true, ops);
      return ops;
    }
    const parent = this.#parent(path, ops);
    this.#writeChild(parent, path, value, true, ops);
    return ops;
  }

  /** Merges `value` into the value at `path`; nothing is deleted. */
  patch(path: Path, value: unknown): PatchOp[] {
    const ops: PatchOp[] = [];
    if (path.length === 0) {
      if (!isPlainObject(value) && !(value instanceof Map)) {
        throw new TypeError("the state root must be an object");
      }
      this.#write(this.#root, path, value, false, ops);
      return ops;
    }
    const parent = this.#parent(path, ops);
    this.#writeChild(parent, path, value, false, ops);
    return ops;
  }

  /** Deletes the value at `path`, if any. */
  delete(path: Path): PatchOp[] {
    if (path.length === 0) {
      const ops: PatchOp[] = [...this.#root.keys()].map((k) => ({
        p: [k],
        del: true,
      }));
      this.#root.clear();
      return ops;
    }
    const parent = this.#find(path.slice(0, -1));
    if (!(parent instanceof Map) || !parent.has(path.at(-1)!)) return [];
    parent.delete(path.at(-1)!);
    return [{ p: path, del: true }];
  }

  /** Every leaf, for a `state.snapshot`. */
  leaves(): PathValue[] {
    const out: PathValue[] = [];
    const walk = (node: Map<PathSegment, Node>, path: Path) => {
      for (const [k, child] of node) {
        const p = [...path, k];
        if (isLeaf(child)) out.push({ p, v: child.leaf });
        else walk(child, p);
      }
    };
    walk(this.#root, []);
    return out;
  }

  #find(path: Path): Node | undefined {
    let node: Node | undefined = this.#root;
    for (const seg of path) {
      if (!(node instanceof Map)) return undefined;
      node = node.get(seg);
    }
    return node;
  }

  // Walks to the parent of `path`, creating branches (and deleting leaves in
  // the way) as needed.
  #parent(path: Path, ops: PatchOp[]): Map<PathSegment, Node> {
    let node = this.#root;
    for (let i = 0; i < path.length - 1; i++) {
      const seg = path[i];
      let child = node.get(seg);
      if (!(child instanceof Map)) {
        if (child !== undefined) {
          ops.push({ p: path.slice(0, i + 1), del: true });
        }
        child = new Map();
        node.set(seg, child);
      }
      node = child;
    }
    return node;
  }

  #writeChild(
    parent: Map<PathSegment, Node>,
    path: Path,
    value: unknown,
    replace: boolean,
    ops: PatchOp[],
  ) {
    const key = path.at(-1)!;
    const old = parent.get(key);
    if (isBranch(value)) {
      let branch: Map<PathSegment, Node>;
      if (old instanceof Map) {
        branch = old;
      } else {
        if (old !== undefined) ops.push({ p: path, del: true });
        branch = new Map();
        parent.set(key, branch);
      }
      this.#write(branch, path, value, replace, ops);
      return;
    }
    if (isLeaf(old) && deepEqual(old.leaf, value)) return;
    if (old instanceof Map) ops.push({ p: path, del: true });
    parent.set(key, { leaf: value });
    ops.push({ p: path, v: value });
  }

  #write(
    branch: Map<PathSegment, Node>,
    path: Path,
    value: Map<unknown, unknown> | Record<string, unknown>,
    replace: boolean,
    ops: PatchOp[],
  ) {
    const next = entries(value);
    if (replace) {
      const keep = new Set(next.map(([k]) => k));
      for (const k of [...branch.keys()]) {
        if (!keep.has(k)) {
          branch.delete(k);
          ops.push({ p: [...path, k], del: true });
        }
      }
    }
    for (const [k, v] of next) {
      if (v === undefined) continue;
      this.#writeChild(branch, [...path, k], v, replace, ops);
    }
  }
}

function toValue(node: Node): unknown {
  if (isLeaf(node)) return node.leaf;
  const out: Record<string, unknown> = {};
  for (const [k, child] of node) out[String(k)] = toValue(child);
  return out;
}

/**
 * Holds patch ops until the next flush, keeping only the latest op per path
 * (spec §6). A delete also drops pending ops below it.
 */
export class PatchCoalescer {
  #pending = new Map<string, PatchOp>();

  get size(): number {
    return this.#pending.size;
  }

  add(op: PatchOp) {
    if ("del" in op) {
      for (const [key, pending] of this.#pending) {
        if (isPrefix(op.p, pending.p)) this.#pending.delete(key);
      }
    }
    // A set never replaces a pending delete of the same path: the delete
    // clears the subtree, the set writes the new value.
    const key = pathKey(op.p) + ("del" in op ? "-" : "=");
    this.#pending.delete(key); // re-insert so ops stay in write order
    this.#pending.set(key, op);
  }

  /** Returns the pending ops in write order and clears them. */
  take(): PatchOp[] {
    const ops = [...this.#pending.values()];
    this.#pending.clear();
    return ops;
  }

  clear() {
    this.#pending.clear();
  }
}
