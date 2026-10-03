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
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));
/** Offsets steeper than this toward the default camera axis would hide one
 *  community behind another in the front overview, so 3D placement avoids them. */
const MAX_VIEW_AXIS_DEPTH = 0.75;

/** Evenly spread unit vectors (Fibonacci sphere), turned by a stable phase. */
function sphereDirections(count: number, phase: number): Point3[] {
  return Array.from({ length: count }, (_, index) => {
    const y = 1 - 2 * (index + 0.5) / count;
    const ring = Math.sqrt(Math.max(0, 1 - y * y));
    const angle = phase + index * GOLDEN_ANGLE;
    return { x: Math.cos(angle) * ring, y, z: Math.sin(angle) * ring };
  });
}

function length3(point: Point3): number {
  return Math.hypot(point.x, point.y, point.z);
}

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
  const parentOf = new Map<string, string>();
  let shell = 0;
  while (frontier.length) {
    const next: string[] = [];
    // Keep siblings consecutive around the ring, so actual branches read as
    // rays. BFS influences coordinates only; it adds no tree edges to the data.
    for (const parent of frontier) {
      for (const child of adjacent(parent).sort(rank)) {
        if (seen.has(child)) continue;
        seen.add(child);
        parentOf.set(child, parent);
        next.push(child);
      }
    }
    if (!next.length) break;
    let offset = 0;
    while (offset < next.length) {
      radius += spacing * (radius === 0 ? 1.15 + density * 0.55 : 1.05);
      if (dimensions === 3) {
        // Hop rings become spherical shells, sized by surface area.
        const capacity = Math.max(6, Math.floor(2 * TAU * radius * radius / (1.3 * spacing * spacing)));
        const members = next.slice(offset, offset + capacity);
        // The hub's own neighbors spread evenly; deeper members choose among all
        // shell slots so each can stay near its parent's direction.
        const fromHub = members.every((id) => parentOf.get(id) === group.hubId);
        placeShell(members, fromHub ? members.length : capacity, radius, phase + shell++, parentOf, group.hubId, positions);
        offset += members.length;
        continue;
      }
      const capacity = Math.max(6, Math.floor(TAU * radius / spacing));
      const ring = next.slice(offset, offset + capacity);
      for (let i = 0; i < ring.length; i++) {
        const angle = phase + TAU * (i + (offset ? 0.5 : 0)) / ring.length;
        positions.set(ring[i], {
          x: Math.cos(angle) * radius,
          y: Math.sin(angle) * radius,
          z: 0,
        });
      }
      offset += ring.length;
    }
    frontier = next;
  }
  group.radius = dimensions === 3
    ? radius + spacing * 0.65
    : Math.hypot(radius, spacing * 0.65) + spacing * 0.65;
}

/**
 * Even shell slots; each member takes the free slot nearest its parent's
 * direction, so real branches still read as rays outward from the hub.
 */
function placeShell(
  members: string[],
  slotCount: number,
  radius: number,
  phase: number,
  parentOf: Map<string, string>,
  hubId: string,
  positions: Map<string, Point3>
): void {
  const slots = sphereDirections(Math.max(slotCount, members.length), phase);
  const free = new Set(slots.keys());
  for (const id of members) {
    const parent = parentOf.get(id);
    const from = parent && parent !== hubId ? positions.get(parent) : undefined;
    const reach = from ? length3(from) : 0;
    let chosen = free.values().next().value as number;
    if (from && reach > 0) {
      let best = -Infinity;
      for (const slot of free) {
        const direction = slots[slot];
        const alignment = (direction.x * from.x + direction.y * from.y + direction.z * from.z) / reach;
        if (alignment > best) {
          best = alignment;
          chosen = slot;
        }
      }
    }
    free.delete(chosen);
    const direction = slots[chosen];
    positions.set(id, { x: direction.x * radius, y: direction.y * radius, z: direction.z * radius });
  }
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
  spacing: number,
  dimensions: 2 | 3
): void {
  const gap = spacing * 2.8;
  const pending = new Set(component.map((group) => group.index));
  const byIndex = new Map(component.map((group) => [group.index, group]));
  const placed: CommunityGroup[] = [];
  const queue = [component[0].index];
  const queued = new Set(queue);
  const separation = (a: Point3, b: Point3) => dimensions === 3
    ? Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z) : Math.hypot(a.x - b.x, a.y - b.y);
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
      // 3D candidates surround the anchor in depth as well, but not along the
      // camera axis where one topic would cover another in the overview.
      const directions = dimensions === 3
        ? sphereDirections(48, phase).filter((direction) => Math.abs(direction.z) <= MAX_VIEW_AXIS_DEPTH)
        : Array.from({ length: 24 }, (_, step) => {
          const angle = phase + TAU * step / 24;
          return { x: Math.cos(angle), y: Math.sin(angle), z: 0 };
        });
      for (let ring = 0; ring < 8 && !best; ring++) {
        for (const anchor of anchors.slice(0, 4)) {
          const distance = anchor.radius + group.radius + gap +
            ring * (group.radius + gap);
          for (const direction of directions) {
            const point = {
              x: anchor.center.x + direction.x * distance,
              y: anchor.center.y + direction.y * distance,
              z: anchor.center.z + direction.z * distance,
            };
            if (placed.some((other) => separation(point, other.center)
              < group.radius + other.radius + gap - spacing * 1e-8)) continue;
            const score = anchors.reduce((sum, other) => sum +
              adjacency[group.index].get(other.index)! / maximumWeight *
              Math.log1p(separation(point, other.center) / spacing), 0) +
              1e-4 * separation(point, { x: 0, y: 0, z: 0 }) / spacing;
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

/** Shelf-pack whole disconnected components by bounding rectangles, not at a
 *  shared origin. The gutters keep unrelated topics farther apart than members. */
function packComponents2D(components: CommunityGroup[][], gutter: number): void {
  const boxes = components.map((component) => {
    const left = Math.min(...component.map((g) => g.center.x - g.radius));
    const top = Math.min(...component.map((g) => g.center.y - g.radius));
    const right = Math.max(...component.map((g) => g.center.x + g.radius));
    const bottom = Math.max(...component.map((g) => g.center.y + g.radius));
    return { component, left, top, width: right - left, height: bottom - top };
  });
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
}

/**
 * Pack disconnected components as bounding spheres around the first one. Each
 * takes the nearest gap along a few even directions, so the whole graph fills
 * a volume rather than a sheet while unrelated topics keep their gutter.
 */
function packComponents3D(components: CommunityGroup[][], gutter: number): void {
  const placed: { center: Point3; radius: number }[] = [];
  for (const component of components) {
    const middle = (axis: "x" | "y" | "z") => component.reduce((sum, g) => sum + g.center[axis], 0) / component.length;
    const centroid = { x: middle("x"), y: middle("y"), z: middle("z") };
    const radius = Math.max(...component.map((g) => Math.hypot(
      g.center.x - centroid.x, g.center.y - centroid.y, g.center.z - centroid.z) + g.radius));
    let target = { x: 0, y: 0, z: 0 };
    if (placed.length) {
      let bestDistance = Infinity;
      const phase = (fnv1a(component[0].hubId) / 0x100000000) * TAU;
      for (const direction of sphereDirections(32, phase)) {
        if (Math.abs(direction.z) > MAX_VIEW_AXIS_DEPTH) continue;
        // Distances along this ray where the sphere would overlap a placed one.
        const blocked = placed.map((other) => {
          const reach = radius + other.radius + gutter;
          const along = direction.x * other.center.x + direction.y * other.center.y + direction.z * other.center.z;
          const clearance = along * along - length3(other.center) ** 2 + reach * reach;
          return clearance > 0 ? [along - Math.sqrt(clearance), along + Math.sqrt(clearance)] : null;
        }).filter((range): range is number[] => range !== null).sort((a, b) => a[0] - b[0]);
        let distance = 0;
        for (const [start, end] of blocked) {
          if (end <= distance) continue;
          if (start > distance) break;
          distance = end;
        }
        if (distance < bestDistance) {
          bestDistance = distance;
          target = { x: direction.x * distance, y: direction.y * distance, z: direction.z * distance };
        }
      }
    }
    for (const group of component) {
      group.center.x += target.x - centroid.x;
      group.center.y += target.y - centroid.y;
      group.center.z += target.z - centroid.z;
    }
    placed.push({ center: target, radius });
  }
}

/**
 * Pure deterministic community targets, in world units. 2D keeps a flat map;
 * 3D places members on spherical shells and communities through depth, with
 * isolates on a surrounding sphere. Titles and supplied degrees are display
 * metadata, never evidence for relationships.
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
    placeConnectedGroups(component, adjacency, spacing, dimensions);
    components.push(component);
  }
  const gutter = spacing * 3.5;
  if (dimensions === 3) packComponents3D(components, gutter);
  else packComponents2D(components, gutter);
  const middle = (axis: "x" | "y" | "z") => groups.length ? (
    Math.min(...groups.map((g) => g.center[axis] - g.radius)) +
    Math.max(...groups.map((g) => g.center[axis] + g.radius))) / 2 : 0;
  const origin = { x: middle("x"), y: middle("y"), z: dimensions === 3 ? middle("z") : 0 };
  for (const group of groups) {
    group.center.x -= origin.x;
    group.center.y -= origin.y;
    group.center.z -= origin.z;
    for (const id of group.nodeIds) {
      const point = positions.get(id)!;
      point.x += group.center.x;
      point.y += group.center.y;
      point.z += group.center.z;
    }
  }
  const isolates = [...assignment.isolatedIds];
  const extent = Math.max(0, ...groups.map((g) => length3(g.center) + g.radius));
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
