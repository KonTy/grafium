/// Shared cluster-coloring logic for the 2D (GraphView.svelte) and 3D
/// (GraphView3D.svelte) graph views. "Cluster" here means connected
/// component: nodes reachable from each other through links share a
/// color, so a densely-linked group of pages reads as one color family
/// instead of every node/edge being a single uniform accent color.

export interface GraphEdgeLike {
  source: string;
  target: string;
}

export interface ClusterAssignment {
  /// Maps a node id to its cluster's index (0 = biggest cluster). Nodes
  /// with no links at all are intentionally left out — see `isolatedIds`.
  clusterIndexById: Map<string, number>;
  /// Nodes with no links to any other node in the current graph. Kept
  /// separate from `clusterIndexById` rather than treated as
  /// one-node clusters so they don't burn through the color palette
  /// without conveying any real structure — callers should render them in
  /// a plain neutral/muted color instead.
  isolatedIds: Set<string>;
}

/// Union-find connected components over the given node ids and edges.
/// Biggest clusters are assigned index 0, 1, 2, ... in descending size
/// order, so the most visually prominent groups get the earliest (most
/// mutually-distinct) palette entries.
export function computeGraphClusters(
  nodeIds: readonly string[],
  edges: readonly GraphEdgeLike[]
): ClusterAssignment {
  const parent = new Map<string, string>();
  for (const id of nodeIds) parent.set(id, id);

  function find(x: string): string {
    let root = x;
    while (parent.get(root) !== root) root = parent.get(root)!;
    let cur = x;
    while (parent.get(cur) !== root) {
      const next = parent.get(cur)!;
      parent.set(cur, root);
      cur = next;
    }
    return root;
  }
  function union(a: string, b: string): void {
    const ra = find(a);
    const rb = find(b);
    if (ra !== rb) parent.set(ra, rb);
  }

  for (const e of edges) {
    if (parent.has(e.source) && parent.has(e.target)) union(e.source, e.target);
  }

  const rootToIds = new Map<string, string[]>();
  for (const id of nodeIds) {
    const root = find(id);
    const ids = rootToIds.get(root) ?? [];
    ids.push(id);
    rootToIds.set(root, ids);
  }

  const clusters = Array.from(rootToIds.values()).sort((a, b) => b.length - a.length);

  const clusterIndexById = new Map<string, number>();
  const isolatedIds = new Set<string>();
  clusters.forEach((ids, clusterIndex) => {
    if (ids.length <= 1) {
      isolatedIds.add(ids[0]);
      return;
    }
    for (const id of ids) clusterIndexById.set(id, clusterIndex);
  });

  return { clusterIndexById, isolatedIds };
}

// 12 evenly-spaced-ish hues, tuned as two matched palettes: vivid/light
// colors that stay legible on a near-black (OLED) background, and a
// darker/more saturated variant of the *same* hues for light themes, where
// the bright dark-theme version would wash out against white.
const CLUSTER_HUES = [212, 355, 145, 32, 268, 168, 8, 95, 285, 52, 195, 325];

export function clusterPalette(isLightTheme: boolean): string[] {
  return isLightTheme
    ? CLUSTER_HUES.map((h) => `hsl(${h}, 62%, 38%)`)
    : CLUSTER_HUES.map((h) => `hsl(${h}, 70%, 64%)`);
}

export function clusterColor(clusterIndex: number, isLightTheme: boolean): string {
  const palette = clusterPalette(isLightTheme);
  return palette[clusterIndex % palette.length];
}
