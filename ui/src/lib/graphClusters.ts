// Shared structural communities for the 2D and 3D views, not connected components.

import { fnv1a, tagHashKey } from "./tagColor";

export interface GraphEdgeLike {
  source: string;
  target: string;
  /** Defaults to 1. Nonpositive/nonfinite weights are ignored for structure. */
  weight?: number;
  /** Suggested links are visual only: they never establish community membership. */
  suggested?: boolean;
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

export interface RealGraphEdge {
  source: string;
  target: string;
  weight: number;
}

/**
 * Canonical undirected real links, shared with the layout. Duplicate/reversed
 * links add their weights; sorting before summing makes permutations identical.
 * A common rescaling avoids overflow without changing modularity. These are
 * structural copies only; callers retain every original link for rendering.
 */
export function realGraphEdges(
  nodeIds: readonly string[],
  edges: readonly GraphEdgeLike[]
): RealGraphEdge[] {
  const known = new Set(nodeIds);
  const valid: RealGraphEdge[] = [];
  let maximum = 0;
  for (const edge of edges) {
    const weight = edge.weight ?? 1;
    if (edge.suggested || edge.source === edge.target ||
        !known.has(edge.source) || !known.has(edge.target) ||
        !Number.isFinite(weight) || weight <= 0) continue;
    const [source, target] = edge.source < edge.target
      ? [edge.source, edge.target] : [edge.target, edge.source];
    valid.push({ source, target, weight });
    maximum = Math.max(maximum, weight);
  }
  valid.sort((a, b) => compareIds(a.source, b.source) ||
    compareIds(a.target, b.target) || a.weight - b.weight);
  const result: RealGraphEdge[] = [];
  for (const edge of valid) {
    const weight = Math.max(Number.MIN_VALUE, edge.weight / maximum);
    const previous = result[result.length - 1];
    if (previous?.source === edge.source && previous.target === edge.target) {
      previous.weight += weight;
    } else {
      result.push({ source: edge.source, target: edge.target, weight });
    }
  }
  return result;
}

function compareIds(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

type Adjacency = Map<number, number>[];

// Louvain: Blondel et al. (2008), https://arxiv.org/abs/0803.0476.
// Deterministic local modularity ascent followed by repeated aggregation.
// Bounded passes suit the graph view's 200–1000 nodes; modularity still has a
// resolution limit and this is a heuristic, not a semantic topic classifier.
function moveLocally(graph: Adjacency): number[] {
  const labels = graph.map((_, i) => i);
  const degree = graph.map((neighbors) =>
    [...neighbors.values()].reduce((sum, weight) => sum + weight, 0));
  const totalDegree = degree.reduce((sum, weight) => sum + weight, 0);
  const totals = [...degree];
  const sizes = graph.map(() => 1);
  for (let pass = 0; pass < 40; pass++) {
    let moved = false;
    for (let step = 0; step < graph.length; step++) {
      const node = pass % 2 === 0 ? step : graph.length - step - 1;
      const current = labels[node];
      const weights = new Map<number, number>();
      for (const [neighbor, weight] of graph[node]) {
        // Aggregated loops contribute twice to degree, but are constant when
        // moving a node, so must not be counted as a link to another community.
        if (neighbor === node) continue;
        const label = labels[neighbor];
        weights.set(label, (weights.get(label) ?? 0) + weight);
      }
      totals[current] = Math.max(0, totals[current] - degree[node]);
      sizes[current]--;
      const score = (label: number) =>
        (weights.get(label) ?? 0) - (degree[node] / totalDegree) * totals[label];
      let best = current;
      let bestScore = score(current);
      const candidates = [...weights.keys()].sort((a, b) => a - b);
      const empty = sizes.findIndex((size) => size === 0);
      if (empty !== -1 && !weights.has(empty)) candidates.push(empty);
      for (const label of candidates) {
        const candidateScore = score(label);
        const tolerance = 1e-12 * Math.max(
          Math.abs(bestScore), Math.abs(candidateScore), Number.MIN_VALUE);
        if (candidateScore > bestScore + tolerance) {
          best = label;
          bestScore = candidateScore;
        }
      }
      labels[node] = best;
      totals[best] += degree[node];
      sizes[best]++;
      moved ||= best !== current;
    }
    if (!moved) break;
  }
  return labels;
}

// Louvain can leave disconnected communities (Traag et al., 2019,
// https://doi.org/10.1038/s41598-019-41695-z). Split those on real links before
// every aggregation. This connectivity safeguard is not the Leiden algorithm.
function connectedCommunities(graph: Adjacency, labels: number[]): number[][] {
  const seen = new Set<number>();
  const groups: number[][] = [];
  for (let start = 0; start < graph.length; start++) {
    if (seen.has(start)) continue;
    const members = [start];
    seen.add(start);
    for (let head = 0; head < members.length; head++) {
      for (const neighbor of graph[members[head]].keys()) {
        if (!seen.has(neighbor) && labels[neighbor] === labels[start]) {
          seen.add(neighbor);
          members.push(neighbor);
        }
      }
    }
    members.sort((a, b) => a - b);
    groups.push(members);
  }
  return groups;
}

/**
 * Weighted multilevel Louvain communities, largest first, with lexicographic
 * member-ID ties. Only nodes without valid real neighbors remain unassigned.
 */
export function computeGraphClusters(
  nodeIds: readonly string[],
  edges: readonly GraphEdgeLike[]
): ClusterAssignment {
  const ids = [...new Set(nodeIds)].sort(compareIds);
  const links = realGraphEdges(ids, edges);
  const connected = new Set(links.flatMap((edge) => [edge.source, edge.target]));
  const isolatedIds = new Set(ids.filter((id) => !connected.has(id)));
  const active = ids.filter((id) => connected.has(id));
  const clusterIndexById = new Map<string, number>();
  if (active.length === 0) return { clusterIndexById, isolatedIds };
  const index = new Map(active.map((id, i) => [id, i]));
  let graph: Adjacency = active.map(() => new Map());
  for (const edge of links) {
    const a = index.get(edge.source)!;
    const b = index.get(edge.target)!;
    graph[a].set(b, edge.weight);
    graph[b].set(a, edge.weight);
  }
  let members = active.map((_, i) => [i]);
  for (let level = 0; level < 20; level++) {
    const communities = connectedCommunities(graph, moveLocally(graph));
    if (communities.length === graph.length) break;
    const labels: number[] = [];
    const nextMembers = communities.map((community, label) =>
      community.flatMap((node) => {
        labels[node] = label;
        return members[node];
      }));
    const aggregated: Adjacency = communities.map(() => new Map());
    // Sum the full symmetric adjacency matrix. Its diagonal therefore stores
    // twice the internal edge weight, preserving total degree and modularity.
    for (let node = 0; node < graph.length; node++) {
      const row = aggregated[labels[node]];
      for (const [neighbor, weight] of graph[node]) {
        const target = labels[neighbor];
        row.set(target, (row.get(target) ?? 0) + weight);
      }
    }
    graph = aggregated;
    members = nextMembers;
  }
  const clusters = members.map((group) => group.map((node) => active[node]).sort(compareIds))
    .sort((a, b) => b.length - a.length || compareIds(a[0], b[0]));
  clusters.forEach((group, clusterIndex) => {
    for (const id of group) clusterIndexById.set(id, clusterIndex);
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

/** Namespace families share a hue; individual topics get stable shade variations. */
export function planetColor(title: string): string {
  const hue = fnv1a(tagHashKey(title)) % 360;
  const shade = fnv1a(title.trim().toLowerCase());
  return `hsl(${hue}, ${64 + shade % 13}%, ${54 + (shade >>> 8) % 13}%)`;
}
