import {
  computeGraphClusters,
  realGraphEdges,
  type GraphEdgeLike,
} from "./graphClusters";
import { fnv1a } from "./tagColor";

export interface Point3 {
  x: number;
  y: number;
  z: number;
}

export interface CommunityGroup {
  index: number;
  hubId: string;
  nodeIds: string[];
  center: Point3;
  /** Conservative full-3D member bound, including padding, in world units. */
  radius: number;
}

export interface CommunityLayout {
  clusterIndexById: Map<string, number>;
  isolatedIds: Set<string>;
  positions: Map<string, Point3>;
  groups: CommunityGroup[];
  hubIds: Set<string>;
}

function safeSpacing(spacing: number): number {
  // Extreme user/slider values must not overflow coordinate/spring arithmetic.
  return Number.isFinite(spacing) && spacing > 0
    ? Math.max(1, Math.min(1e6, spacing)) : 70;
}

function compareIds(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

type Neighbors = Map<string, Map<string, number>>;
const TAU = Math.PI * 2;

/** Real internal links determine the hub and hop rings, never titles or suggestions. */
function placeMembers(
  group: CommunityGroup,
  neighbors: Neighbors,
  dimensions: 2 | 3,
  spacing: number,
  positions: Map<string, Point3>
): void {
  const internal = new Set(group.nodeIds);
  const adjacent = (id: string) => [...neighbors.get(id)!.keys()]
    .filter((other) => internal.has(other));
  const degrees = new Map(group.nodeIds.map((id) => [id, adjacent(id).length]));
  const strengths = new Map(group.nodeIds.map((id) => [id,
    adjacent(id).reduce((sum, other) => sum + neighbors.get(id)!.get(other)!, 0)]));
  const rank = (a: string, b: string) =>
    degrees.get(b)! - degrees.get(a)! ||
    strengths.get(b)! - strengths.get(a)! || compareIds(a, b);
  group.hubId = [...group.nodeIds].sort(rank)[0];
  positions.set(group.hubId, { x: 0, y: 0, z: 0 });
  const seen = new Set([group.hubId]);
  let frontier = [group.hubId];
  let radius = 0;
  const density = group.nodeIds.length > 1
    ? [...degrees.values()].reduce((a, b) => a + b, 0) /
      (group.nodeIds.length * (group.nodeIds.length - 1)) : 0;
  const phase = (fnv1a(group.hubId) / 0x100000000) * TAU;
  while (frontier.length) {
    const next: string[] = [];
    // Keep siblings consecutive around the ring, so actual branches read as
    // rays. BFS influences coordinates only; it adds no tree edges to the data.
    for (const parent of frontier) {
      for (const child of adjacent(parent).sort(rank)) {
        if (seen.has(child)) continue;
        seen.add(child);
        next.push(child);
      }
    }
    if (!next.length) break;
    let offset = 0;
    while (offset < next.length) {
      radius += spacing * (radius === 0 ? 1.15 + density * 0.55 : 1.05);
      const capacity = Math.max(6, Math.floor(TAU * radius / spacing));
      const ring = next.slice(offset, offset + capacity);
      for (let i = 0; i < ring.length; i++) {
        const angle = phase + TAU * (i + (offset ? 0.5 : 0)) / ring.length;
        const depth = spacing * 0.65 * Math.sin(angle * 2 + phase);
        positions.set(ring[i], {
          x: Math.cos(angle) * radius,
          y: Math.sin(angle) * radius,
          z: dimensions === 3 ? depth : 0,
        });
      }
      offset += ring.length;
    }
    frontier = next;
  }
  // Same bound in both dimensions keeps the coarse XY map consistent.
  group.radius = Math.hypot(radius, spacing * 0.65) + spacing * 0.65;
}

/**
 * Place a connected coarse graph using only its real bridges. Greedy candidates
 * near already placed neighbors minimize weighted log-distance; then hard disk
 * separation prevents even dense cross-links from collapsing topic boundaries.
 *
 * Log attraction follows the cluster-readable motivation of ForceAtlas2 LinLog
 * (https://doi.org/10.1371/journal.pone.0098679), not a reimplementation of its
 * force solver. These are stable structural targets for either renderer.
 */
function placeConnectedGroups(
  component: CommunityGroup[],
  adjacency: Map<number, number>[],
  spacing: number
): void {
  const gap = spacing * 2.8;
  const pending = new Set(component.map((group) => group.index));
  const byIndex = new Map(component.map((group) => [group.index, group]));
  const placed: CommunityGroup[] = [];
  const queue = [component[0].index];
  const queued = new Set(queue);
  for (let head = 0; head < queue.length; head++) {
    const group = byIndex.get(queue[head])!;
    pending.delete(group.index);
    if (placed.length) {
      const anchors = placed.filter((other) => adjacency[group.index].has(other.index))
        .sort((a, b) => adjacency[group.index].get(b.index)! -
          adjacency[group.index].get(a.index)! || a.index - b.index);
      let best: Point3 | undefined;
      let bestScore = Infinity;
      const maximumWeight = adjacency[group.index].get(anchors[0].index)!;
      const phase = (fnv1a(group.hubId) / 0x100000000) * TAU;
      for (let ring = 0; ring < 8 && !best; ring++) {
        for (const anchor of anchors.slice(0, 4)) {
          const distance = anchor.radius + group.radius + gap +
            ring * (group.radius + gap);
          for (let step = 0; step < 24; step++) {
            const angle = phase + TAU * step / 24;
            const point = {
              x: anchor.center.x + Math.cos(angle) * distance,
              y: anchor.center.y + Math.sin(angle) * distance,
              z: 0,
            };
            if (placed.some((other) => Math.hypot(
              point.x - other.center.x, point.y - other.center.y
            ) < group.radius + other.radius + gap - spacing * 1e-8)) continue;
            const score = anchors.reduce((sum, other) => sum +
              adjacency[group.index].get(other.index)! / maximumWeight *
              Math.log1p(Math.hypot(point.x - other.center.x,
                point.y - other.center.y) / spacing), 0) +
              1e-4 * Math.hypot(point.x, point.y) / spacing;
            if (score < bestScore) {
              best = point;
              bestScore = score;
            }
          }
        }
      }
      // A bounded search must still guarantee separation in crowded cases.
      group.center = best ?? {
        x: Math.max(...placed.map((other) => other.center.x + other.radius)) +
          group.radius + gap,
        y: 0,
        z: 0,
      };
    }
    placed.push(group);
    const adjacent = [...adjacency[group.index].entries()]
      .sort((a, b) => b[1] - a[1] || a[0] - b[0]);
    for (const [other] of adjacent) {
      if (pending.has(other) && !queued.has(other)) {
        queued.add(other);
        queue.push(other);
      }
    }
  }
}

/**
 * Pure deterministic community targets, in world units. The same spacing gives
 * the same community XY map in 2D/3D; 3D adds bounded depth within each community
 * and places isolates on a surrounding sphere. Titles and supplied degrees are
 * display metadata, never evidence for relationships.
 */
export function createCommunityLayout(
  nodes: readonly { id: string; title: string; degree?: number }[],
  edges: readonly GraphEdgeLike[],
  dimensions: 2 | 3,
  spacing = 70
): CommunityLayout {
  spacing = safeSpacing(spacing);
  const ids = [...new Set(nodes.map((node) => node.id))].sort(compareIds);
  const assignment = computeGraphClusters(ids, edges);
  const links = realGraphEdges(ids, edges);
  const positions = new Map<string, Point3>();
  const groups: CommunityGroup[] = [];
  const neighbors: Neighbors = new Map(ids.map((id) => [id, new Map()]));
  for (const edge of links) {
    neighbors.get(edge.source)!.set(edge.target, edge.weight);
    neighbors.get(edge.target)!.set(edge.source, edge.weight);
  }
  for (const [id, index] of assignment.clusterIndexById) {
    groups[index] ??= { index, hubId: id, nodeIds: [], center: { x: 0, y: 0, z: 0 }, radius: 0 };
    groups[index].nodeIds.push(id);
  }
  for (const group of groups) placeMembers(group, neighbors, dimensions, spacing, positions);
  const adjacency: Map<number, number>[] = groups.map(() => new Map());
  for (const edge of links) {
    const source = assignment.clusterIndexById.get(edge.source)!;
    const target = assignment.clusterIndexById.get(edge.target)!;
    if (source === target) continue;
    adjacency[source].set(target, (adjacency[source].get(target) ?? 0) + edge.weight);
    adjacency[target].set(source, (adjacency[target].get(source) ?? 0) + edge.weight);
  }
  const seen = new Set<number>();
  const components: CommunityGroup[][] = [];
  for (const group of groups) {
    if (seen.has(group.index)) continue;
    const component = [group];
    seen.add(group.index);
    for (let head = 0; head < component.length; head++) {
      for (const index of adjacency[component[head].index].keys()) {
        if (seen.has(index)) continue;
        seen.add(index);
        component.push(groups[index]);
      }
    }
    component.sort((a, b) => a.index - b.index);
    placeConnectedGroups(component, adjacency, spacing);
    components.push(component);
  }
  // Pack whole disconnected components by bounding rectangles, not at a shared
  // origin. The gutters also keep unrelated topics farther apart than members.
  const boxes = components.map((component) => {
    const left = Math.min(...component.map((g) => g.center.x - g.radius));
    const top = Math.min(...component.map((g) => g.center.y - g.radius));
    const right = Math.max(...component.map((g) => g.center.x + g.radius));
    const bottom = Math.max(...component.map((g) => g.center.y + g.radius));
    return { component, left, top, width: right - left, height: bottom - top };
  });
  const gutter = spacing * 3.5;
  const shelfWidth = Math.max(0, ...boxes.map((b) => b.width),
    Math.sqrt(boxes.reduce((sum, b) => sum + (b.width + gutter) * (b.height + gutter), 0)));
  let x = 0;
  let y = 0;
  let rowHeight = 0;
  for (const box of boxes) {
    if (x && x + box.width > shelfWidth) {
      x = 0;
      y += rowHeight + gutter;
      rowHeight = 0;
    }
    for (const group of box.component) {
      group.center.x += x - box.left;
      group.center.y += y - box.top;
    }
    x += box.width + gutter;
    rowHeight = Math.max(rowHeight, box.height);
  }
  const originX = groups.length ? (
    Math.min(...groups.map((g) => g.center.x - g.radius)) +
    Math.max(...groups.map((g) => g.center.x + g.radius))) / 2 : 0;
  const originY = groups.length ? (
    Math.min(...groups.map((g) => g.center.y - g.radius)) +
    Math.max(...groups.map((g) => g.center.y + g.radius))) / 2 : 0;
  for (const group of groups) {
    group.center.x -= originX;
    group.center.y -= originY;
    for (const id of group.nodeIds) {
      const point = positions.get(id)!;
      point.x += group.center.x;
      point.y += group.center.y;
    }
  }
  const isolates = [...assignment.isolatedIds];
  const extent = Math.max(0, ...groups.map((g) => Math.hypot(g.center.x, g.center.y) + g.radius));
  let isolateRadius = Math.max(extent + spacing * 2.5, spacing * Math.sqrt(isolates.length));
  if (dimensions === 3) {
    // Fibonacci sphere: equal-area latitude bands and golden-angle longitude
    // spread points over the full surface without crowding the poles.
    const goldenAngle = Math.PI * (3 - Math.sqrt(5));
    isolates.forEach((id, index) => {
      const y = 1 - 2 * (index + 0.5) / isolates.length;
      const ringRadius = Math.sqrt(Math.max(0, 1 - y * y));
      const angle = index * goldenAngle;
      positions.set(id, {
        x: Math.cos(angle) * ringRadius * isolateRadius,
        y: y * isolateRadius,
        z: Math.sin(angle) * ringRadius * isolateRadius,
      });
    });
  } else {
    let offset = 0;
    while (offset < isolates.length) {
      const capacity = Math.max(6, Math.floor(TAU * isolateRadius / spacing));
      const ring = isolates.slice(offset, offset + capacity);
      for (let i = 0; i < ring.length; i++) {
        const angle = TAU * (i + (offset ? 0.5 : 0)) / ring.length;
        positions.set(ring[i], {
          x: Math.cos(angle) * isolateRadius,
          y: Math.sin(angle) * isolateRadius,
          z: 0,
        });
      }
      offset += ring.length;
      isolateRadius += spacing * 1.2;
    }
  }
  return { ...assignment, positions, groups, hubIds: new Set(groups.map((g) => g.hubId)) };
}

/** Short local links and long bridges; this does not create or remove any edge. */
export function communityLinkDistance(
  layout: CommunityLayout,
  sourceId: string,
  targetId: string,
  spacing = 70
): number {
  spacing = safeSpacing(spacing);
  const source = layout.positions.get(sourceId);
  const target = layout.positions.get(targetId);
  const a = layout.clusterIndexById.get(sourceId);
  const b = layout.clusterIndexById.get(targetId);
  const seeded = source && target
    ? Math.hypot(source.x - target.x, source.y - target.y, source.z - target.z) : spacing;
  if (a !== undefined && b !== undefined && a !== b) {
    const first = layout.groups[a];
    const second = layout.groups[b];
    const centers = Math.hypot(first.center.x - second.center.x,
      first.center.y - second.center.y, first.center.z - second.center.z);
    return Math.max(spacing * 4, seeded, centers,
      first.radius + second.radius + spacing * 2.8);
  }
  if (a === undefined || b === undefined) return Math.max(spacing * 1.5, seeded);
  return Math.max(spacing * 0.85, seeded);
}
