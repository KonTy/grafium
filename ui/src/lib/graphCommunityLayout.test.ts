import { describe, expect, it } from "vitest";
import { computeGraphClusters, type GraphEdgeLike } from "./graphClusters";
import {
  communityLinkDistance, createCommunityLayout, type CommunityLayout,
} from "./graphCommunityLayout";

const nodes = (ids: string[]) => ids.map((id) => ({ id, title: id }));

function topics(sizes: number[], bridge = true) {
  const groups = sizes.map((size, i) =>
    Array.from({ length: size }, (_, j) => `topic${i}-${String(j).padStart(3, "0")}`));
  const edges: GraphEdgeLike[] = groups.flatMap((ids) =>
    ids.flatMap((source, i) => ids.slice(i + 1).map((target) => ({ source, target }))));
  if (bridge) {
    for (let i = 1; i < groups.length; i++) {
      edges.push({ source: groups[i - 1][0], target: groups[i][0] });
    }
  }
  return { nodes: nodes(groups.flat()), edges, groups };
}

function verifyBounds(layout: CommunityLayout, spacing: number, dimensions: 2 | 3) {
  for (const point of layout.positions.values()) {
    expect(Number.isFinite(point.x) && Number.isFinite(point.y) && Number.isFinite(point.z)).toBe(true);
    if (dimensions === 2) expect(point.z).toBe(0);
  }
  for (const group of layout.groups) {
    expect(group.nodeIds).toContain(group.hubId);
    expect(layout.hubIds.has(group.hubId)).toBe(true);
    expect(layout.positions.get(group.hubId)).toEqual(group.center);
    for (const id of group.nodeIds) {
      const point = layout.positions.get(id)!;
      expect(Math.hypot(point.x - group.center.x, point.y - group.center.y,
        point.z - group.center.z)).toBeLessThan(group.radius);
      expect(layout.clusterIndexById.get(id)).toBe(group.index);
    }
    for (const other of layout.groups) {
      if (other.index <= group.index) continue;
      expect(Math.hypot(other.center.x - group.center.x, other.center.y - group.center.y))
        .toBeGreaterThanOrEqual(group.radius + other.radius + spacing * 2.79);
    }
    for (const id of layout.isolatedIds) {
      const point = layout.positions.get(id)!;
      expect(Math.hypot(point.x - group.center.x, point.y - group.center.y, point.z - group.center.z))
        .toBeGreaterThan(group.radius + spacing * 2);
    }
  }
}

describe("createCommunityLayout", () => {
  it.each([2, 3] as const)("separates bridge-linked topic bounds in %iD without dropping links or nodes", (dimensions) => {
    const graph = topics([8, 8]);
    const before = JSON.stringify(graph);
    const layout = createCommunityLayout(graph.nodes, graph.edges, dimensions);
    expect(layout.groups).toHaveLength(2);
    expect(layout.positions.size).toBe(graph.nodes.length);
    expect(layout.clusterIndexById).toEqual(computeGraphClusters(graph.nodes.map((n) => n.id), graph.edges).clusterIndexById);
    expect(JSON.stringify(graph)).toBe(before);
    verifyBounds(layout, 70, dimensions);
    const bridge = graph.edges[graph.edges.length - 1];
    const inside = graph.edges[0];
    expect(communityLinkDistance(layout, bridge.source, bridge.target)).toBeGreaterThan(
      communityLinkDistance(layout, inside.source, inside.target) * 2);
  });

  it.each([2, 3] as const)("puts a real star hub at the center with distinct radial leaves in %iD", (dimensions) => {
    const ids = ["hub", ...Array.from({ length: 24 }, (_, i) => `leaf${i}`)];
    const edges = ids.slice(1).map((target) => ({ source: "hub", target }));
    const graphNodes = nodes(ids).map((node) => ({ ...node, degree: node.id === "leaf0" ? 9999 : 0 }));
    const layout = createCommunityLayout(graphNodes, edges, dimensions);
    expect(layout.groups).toHaveLength(1);
    expect(layout.groups[0].hubId).toBe("hub");
    expect(new Set([...layout.positions.values()].map((p) => `${p.x},${p.y},${p.z}`)).size).toBe(ids.length);
    for (const leaf of ids.slice(1)) {
      expect(communityLinkDistance(layout, "hub", leaf)).toBeGreaterThan(70);
    }
    verifyBounds(layout, 70, dimensions);
  });

  it("keeps deeper real branches on outer hop rings", () => {
    const ids = ["hub", "arm0", "arm1", "arm2", "arm3", "leaf0", "leaf1", "leaf2", "leaf3"];
    const edges = Array.from({ length: 4 }, (_, i) => [
      { source: "hub", target: `arm${i}`, weight: 20 },
      { source: `arm${i}`, target: `leaf${i}` },
    ]).flat();
    const layout = createCommunityLayout(nodes(ids), edges, 2);
    expect(layout.groups).toHaveLength(1);
    expect(layout.groups[0].hubId).toBe("hub");
    const center = layout.positions.get("hub")!;
    for (let i = 0; i < 4; i++) {
      const arm = layout.positions.get(`arm${i}`)!;
      const leaf = layout.positions.get(`leaf${i}`)!;
      expect(Math.hypot(leaf.x - center.x, leaf.y - center.y))
        .toBeGreaterThan(Math.hypot(arm.x - center.x, arm.y - center.y));
    }
  });

  it("uses the same XY structure and bounds in 2D and 3D, with useful bounded depth", () => {
    const graph = topics([6, 7, 10]);
    const flat = createCommunityLayout(graph.nodes, graph.edges, 2);
    const deep = createCommunityLayout(graph.nodes, graph.edges, 3);
    expect(deep.groups).toEqual(flat.groups);
    expect(deep.clusterIndexById).toEqual(flat.clusterIndexById);
    for (const [id, point] of flat.positions) {
      expect(deep.positions.get(id)!.x).toBe(point.x);
      expect(deep.positions.get(id)!.y).toBe(point.y);
    }
    expect([...deep.positions.values()].some((p) => Math.abs(p.z) > 1)).toBe(true);
    verifyBounds(deep, 70, 3);
  });

  it("places disconnected unequal communities separately and isolates outside them", () => {
    const graph = topics([3, 17, 5, 8], false);
    graph.nodes.push(...nodes(["isolated-a", "isolated-b", "isolated-c"]));
    const layout = createCommunityLayout(graph.nodes, graph.edges, 2);
    expect(layout.groups.map((g) => g.nodeIds.length)).toEqual([17, 8, 5, 3]);
    expect(layout.groups[0].radius).toBeGreaterThan(layout.groups[3].radius);
    expect(layout.isolatedIds.size).toBe(3);
    expect(layout.positions.size).toBe(graph.nodes.length);
    verifyBounds(layout, 70, 2);
  });

  it("spreads 3D isolates evenly across a sphere, not a plane or latitude ring", () => {
    const graph = topics([8, 12]);
    graph.nodes.push(...nodes(Array.from({ length: 100 }, (_, i) => `isolated-${i}`)));
    const layout = createCommunityLayout(graph.nodes, graph.edges, 3);
    const points = [...layout.isolatedIds].map((id) => layout.positions.get(id)!);
    const radius = Math.hypot(points[0].x, points[0].y, points[0].z);
    for (const point of points) {
      expect(Math.hypot(point.x, point.y, point.z)).toBeCloseTo(radius, 8);
    }
    for (const axis of ["x", "y", "z"] as const) {
      const values = points.map((point) => point[axis] / radius);
      expect(Math.min(...values)).toBeLessThan(-0.9);
      expect(Math.max(...values)).toBeGreaterThan(0.9);
      expect(Math.abs(values.reduce((sum, value) => sum + value, 0) / values.length)).toBeLessThan(0.02);
      expect(values.reduce((sum, value) => sum + value * value, 0) / values.length).toBeCloseTo(1 / 3, 2);
    }
    for (const [a, b] of [["x", "y"], ["x", "z"], ["y", "z"]] as const) {
      expect(Math.abs(points.reduce((sum, point) => sum + point[a] * point[b], 0) /
        points.length / radius ** 2)).toBeLessThan(0.02);
    }
    for (let i = 0; i < points.length; i++) {
      for (const other of points.slice(i + 1)) {
        expect(Math.hypot(points[i].x - other.x, points[i].y - other.y, points[i].z - other.z))
          .toBeGreaterThan(70);
      }
    }
    expect(createCommunityLayout([...graph.nodes].reverse(), [...graph.edges].reverse(), 3)).toEqual(layout);
    verifyBounds(layout, 70, 3);
    const flat = createCommunityLayout(graph.nodes, graph.edges, 2);
    expect(flat.groups).toEqual(layout.groups);
    expect(flat.clusterIndexById).toEqual(layout.clusterIndexById);
    expect(flat.isolatedIds).toEqual(layout.isolatedIds);
    expect([...flat.positions.values()].every((point) => point.z === 0)).toBe(true);
  });

  it.each([1, 2, 3])("places %i isolated nodes at distinct finite sphere positions", (count) => {
    const graphNodes = nodes(Array.from({ length: count }, (_, i) => `isolated-${i}`));
    const layout = createCommunityLayout(graphNodes, [], 3);
    expect(layout.positions.size).toBe(count);
    expect(new Set([...layout.positions.values()].map((point) => JSON.stringify(point))).size).toBe(count);
    verifyBounds(layout, 70, 3);
  });

  it("follows coarse bridge adjacency rather than community index order", () => {
    const graph = topics([8, 8, 8, 8], false);
    const [a, b, c, d] = graph.groups;
    graph.edges.push({ source: a[0], target: d[0] }, { source: d[0], target: b[0] },
      { source: b[0], target: c[0] });
    const layout = createCommunityLayout(graph.nodes, graph.edges, 2);
    expect(layout.groups).toHaveLength(4);
    const [first, , , fourth] = layout.groups;
    const actualDistance = Math.hypot(first.center.x - fourth.center.x,
      first.center.y - fourth.center.y);
    expect(actualDistance).toBeCloseTo(first.radius + fourth.radius + 70 * 2.8);
    verifyBounds(layout, 70, 2);
  });

  it("is deterministic under reordered/reversed edges, nodes, and misleading title metadata", () => {
    const graph = topics([5, 7, 9]);
    const baseline = createCommunityLayout(graph.nodes, graph.edges, 3);
    const reordered = createCommunityLayout([...graph.nodes].reverse()
      .map((n) => ({ ...n, title: "unrelated title", degree: Infinity })),
    [...graph.edges].reverse().map((e) => ({ ...e, source: e.target, target: e.source })), 3);
    expect(reordered).toEqual(baseline);
  });

  it("ignores suggestions, invalid weights and unknown/self endpoints for every structural decision", () => {
    const graph = topics([5, 8], false);
    graph.nodes.push(...nodes(["alone"]));
    const baseline = createCommunityLayout(graph.nodes, graph.edges, 3);
    const contaminated = [
      ...graph.edges,
      { source: graph.nodes[0].id, target: "alone", suggested: true, weight: 1e12 },
      { source: graph.nodes[0].id, target: graph.nodes[6].id, suggested: true },
      { source: "alone", target: "alone" },
      { source: "alone", target: "ghost" },
      ...[0, -1, NaN, Infinity].map((weight) => ({ source: graph.nodes[0].id, target: "alone", weight })),
    ];
    expect(createCommunityLayout(graph.nodes, contaminated, 3)).toEqual(baseline);
  });

  it.each([2, 3] as const)("supports empty, all-isolated and single ambiguous community graphs in %iD", (dimensions) => {
    const empty = createCommunityLayout([], [], dimensions);
    expect(empty.positions.size).toBe(0);
    expect(empty.groups).toEqual([]);
    const isolated = createCommunityLayout(nodes(Array.from({ length: 100 }, (_, i) => `n${i}`)), [], dimensions);
    expect(isolated.groups).toEqual([]);
    expect(isolated.positions.size).toBe(100);
    expect(isolated.isolatedIds.size).toBe(100);
    verifyBounds(isolated, 70, dimensions);
    const graph = topics([30]);
    const single = createCommunityLayout(graph.nodes, graph.edges, dimensions);
    expect(single.groups).toHaveLength(1);
    verifyBounds(single, 70, dimensions);
  });

  it("scales linearly between viewport units and guards invalid/extreme spacing", () => {
    const graph = topics([5, 9]);
    const small = createCommunityLayout(graph.nodes, graph.edges, 3, 70);
    const large = createCommunityLayout(graph.nodes, graph.edges, 3, 180);
    for (const [id, point] of small.positions) {
      const scaled = large.positions.get(id)!;
      expect(scaled.x).toBeCloseTo(point.x * 180 / 70);
      expect(scaled.y).toBeCloseTo(point.y * 180 / 70);
      expect(scaled.z).toBeCloseTo(point.z * 180 / 70);
    }
    for (const spacing of [0, -1, NaN, Infinity, Number.MAX_VALUE, Number.MIN_VALUE]) {
      const layout = createCommunityLayout(graph.nodes, graph.edges, 3, spacing);
      for (const point of layout.positions.values()) {
        expect(Object.values(point).every(Number.isFinite)).toBe(true);
      }
      expect(Number.isFinite(communityLinkDistance(layout, graph.nodes[0].id, graph.nodes[6].id, spacing))).toBe(true);
    }
    expect(communityLinkDistance(small, "missing", "unknown")).toBe(105);
  });

  it.each([200, 1000])("benchmarks %i nodes with meaningful separated communities", (size) => {
    const graph = topics(Array.from({ length: size / 10 }, () => 10));
    const start = performance.now();
    const layout = createCommunityLayout(graph.nodes, graph.edges, 3);
    const elapsed = performance.now() - start;
    console.info(`community layout: ${size} nodes / ${graph.edges.length} edges / ${layout.groups.length} groups: ${elapsed.toFixed(1)}ms`);
    expect(layout.positions.size).toBe(size);
    expect(layout.groups.length).toBeGreaterThan(2);
    expect(layout.groups.length).toBeLessThan(size / 2);
    verifyBounds(layout, 70, 3);
  });
});
