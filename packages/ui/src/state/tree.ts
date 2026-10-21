import { comparePaths, formatPath, splitPath, type Segment } from "../live/paths";
import type { Leaf, Quality } from "../live/protocol";

export interface TreeNode {
  /** Text path of this node. */
  path: string;
  /** How it's shown: `outputs`, `[3]` or `["SDI 4"]`. */
  label: string;
  leaf?: Leaf;
  children: TreeNode[];
  /** The leaf's quality, or the worst quality below it. */
  quality: Quality;
  /** Leaves at or below this node. */
  count: number;
}

/** Worst first: one stale leaf makes its whole branch read as stale. */
const rank: Record<Quality, number> = { stale: 2, unknown: 1, good: 0 };

/** Builds the tree of `leaves` under `root` (a text path). */
export function buildTree(root: string, leaves: readonly Leaf[]): TreeNode {
  const base = splitPath(root);
  const top: TreeNode = { path: root, label: root, children: [], quality: "good", count: 0 };
  const index = new Map<string, TreeNode>([[root, top]]);

  for (const leaf of leaves) {
    const { segments } = splitPath(leaf.path);
    let node = top;
    for (let i = base.segments.length; i < segments.length; i++) {
      const path = formatPath(base.root, segments.slice(0, i + 1));
      let child = index.get(path);
      if (child === undefined) {
        child = { path, label: labelOf(segments[i]!), children: [], quality: "good", count: 0 };
        index.set(path, child);
        node.children.push(child);
      }
      node = child;
    }
    node.leaf = leaf;
  }
  summarise(top);
  return top;
}

function labelOf(segment: Segment): string {
  return formatPath("", [segment]).replace(/^\./, "");
}

function summarise(node: TreeNode) {
  node.children.sort((a, b) => comparePaths(a.path, b.path));
  let quality: Quality = node.leaf?.quality ?? "good";
  let count = node.leaf === undefined ? 0 : 1;
  for (const child of node.children) {
    summarise(child);
    if (rank[child.quality] > rank[quality]) quality = child.quality;
    count += child.count;
  }
  node.quality = quality;
  node.count = count;
}

export interface Row {
  node: TreeNode;
  depth: number;
}

/** The rows on screen: every node whose ancestors are all open. The root itself isn't a row. */
export function visibleRows(root: TreeNode, collapsed: ReadonlySet<string>): Row[] {
  const rows: Row[] = [];
  const walk = (node: TreeNode, depth: number) => {
    for (const child of node.children) {
      rows.push({ node: child, depth });
      if (!collapsed.has(child.path)) walk(child, depth + 1);
    }
  };
  walk(root, 0);
  return rows;
}
