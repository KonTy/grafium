import { describe, expect, it } from "vitest";
import {
  computeGraphClusters, clusterColor, clusterPalette, planetColor, realGraphEdges,
  type GraphEdgeLike,
} from "./graphClusters";

function clique(ids: string[], weight = 1): GraphEdgeLike[] {
  return ids.flatMap((source, i) => ids.slice(i + 1).map((target) => ({ source, target, weight })));
}

function modularity(ids: string[], edges: GraphEdgeLike[], labels: Map<string, number>): number {
  const links = realGraphEdges(ids, edges);
  const total = links.reduce((sum, e) => sum + e.weight, 0);
  const degrees = new Map<number, number>();
  let internal = 0;
  for (const edge of links) {
    const a = labels.get(edge.source)!;
    const b = labels.get(edge.target)!;
    degrees.set(a, (degrees.get(a) ?? 0) + edge.weight);
    degrees.set(b, (degrees.get(b) ?? 0) + edge.weight);
    if (a === b) internal += edge.weight;
  }
  return internal / total - [...degrees.values()].reduce((sum, k) => sum + (k / (2 * total)) ** 2, 0);
}

describe("computeGraphClusters", () => {
  it("groups a hub and leaves together and orders communities by size", () => {
    // Cluster A: a-1..a-4 (a star), Cluster B: b-1, b-2, isolated: c-1
    const nodeIds = ["a-1", "a-2", "a-3", "a-4", "b-1", "b-2", "c-1"];
    const edges = [
      { source: "a-1", target: "a-2" },
      { source: "a-1", target: "a-3" },
      { source: "a-1", target: "a-4" },
      { source: "b-1", target: "b-2" },
    ];

    const { clusterIndexById, isolatedIds } = computeGraphClusters(nodeIds, edges);

    expect(isolatedIds.has("c-1")).toBe(true);
    expect(clusterIndexById.has("c-1")).toBe(false);

    // The 4-node cluster must be index 0 (biggest first).
    const aIndex = clusterIndexById.get("a-1");
    const bIndex = clusterIndexById.get("b-1");
    expect(aIndex).toBe(0);
    expect(bIndex).toBe(1);

    // All members of the same cluster share the same index.
    for (const id of ["a-1", "a-2", "a-3", "a-4"]) {
      expect(clusterIndexById.get(id)).toBe(aIndex);
    }
    expect(clusterIndexById.get("b-2")).toBe(bIndex);
  });

  it("treats every node as isolated when there are no edges", () => {
    const nodeIds = ["x", "y", "z"];
    const { clusterIndexById, isolatedIds } = computeGraphClusters(nodeIds, []);
    expect(isolatedIds.size).toBe(3);
    expect(clusterIndexById.size).toBe(0);
  });

  it("ignores edges referencing ids outside the node set", () => {
    const nodeIds = ["a", "b"];
    const edges = [{ source: "a", target: "ghost" }];
    const { clusterIndexById, isolatedIds } = computeGraphClusters(nodeIds, edges);
    expect(isolatedIds.has("a")).toBe(true);
    expect(isolatedIds.has("b")).toBe(true);
    expect(clusterIndexById.size).toBe(0);
  });

  it("splits dense topics despite multiple cross-links, not just cut bridges", () => {
    const a = ["a0", "a1", "a2", "a3", "a4"];
    const b = ["b0", "b1", "b2", "b3", "b4"];
    const edges = [...clique(a), ...clique(b),
      { source: "a0", target: "b0" }, { source: "a1", target: "b1" }];
    const { clusterIndexById } = computeGraphClusters([...a, ...b], edges);
    for (const id of a) expect(clusterIndexById.get(id)).toBe(0);
    for (const id of b) expect(clusterIndexById.get(id)).toBe(1);
    expect(modularity([...a, ...b], edges, clusterIndexById)).toBeGreaterThan(0.4);
  });

  it("aggregates beyond the first local pairing without losing internal loop mass", () => {
    const ids = Array.from({ length: 12 }, (_, i) => `n${String(i).padStart(2, "0")}`);
    const edges = ids.map((source, i) => ({ source, target: ids[(i + 1) % ids.length] }));
    const result = computeGraphClusters(ids, edges);
    // The first local pass forms pairs (Q=1/3). Aggregated movement must improve
    // on that partition; treating internal loops as ordinary links corrupts Q.
    expect(modularity(ids, edges, result.clusterIndexById)).toBeGreaterThan(0.38);
    expect(new Set(result.clusterIndexById.values()).size).toBeLessThan(6);
    expect(new Set(result.clusterIndexById.values()).size).toBeGreaterThan(1);
  });

  it("respects edge weights and adds duplicate/reversed links", () => {
    const ids = ["a", "b", "c", "d"];
    const edges = [
      { source: "a", target: "b", weight: 20 },
      { source: "c", target: "d", weight: 20 },
      { source: "a", target: "c" }, { source: "b", target: "d" },
    ];
    const weighted = computeGraphClusters(ids, edges);
    expect(weighted.clusterIndexById.get("a")).toBe(weighted.clusterIndexById.get("b"));
    expect(weighted.clusterIndexById.get("a")).not.toBe(weighted.clusterIndexById.get("c"));
    const expanded = [
      ...Array.from({ length: 20 }, (_, i) =>
        ({ source: i % 2 ? "b" : "a", target: i % 2 ? "a" : "b" })),
      ...Array.from({ length: 20 }, () => ({ source: "d", target: "c" })),
      { source: "a", target: "c" }, { source: "b", target: "d" },
    ];
    expect(computeGraphClusters(ids, expanded)).toEqual(weighted);
    const flipped = computeGraphClusters(ids, edges.map((e) => ({ ...e, weight: e.weight ? 1 : 20 })));
    expect(flipped.clusterIndexById.get("a")).toBe(flipped.clusterIndexById.get("c"));
    expect(flipped.clusterIndexById.get("a")).not.toBe(flipped.clusterIndexById.get("b"));
  });

  it("is invariant to node/edge permutations, orientations and duplicate node IDs", () => {
    const ids = ["a0", "a1", "a2", "b0", "b1", "b2", "isolated"];
    const edges = [...clique(ids.slice(0, 3)), ...clique(ids.slice(3, 6)),
      { source: "a0", target: "b0", weight: 0.4 },
      { source: "a0", target: "b0", weight: 0.7 }];
    const baseline = computeGraphClusters(ids, edges);
    const reversed = computeGraphClusters([...ids].reverse().concat(ids[0]),
      [...edges].reverse().map((e) => ({ ...e, source: e.target, target: e.source })));
    expect([...reversed.clusterIndexById]).toEqual([...baseline.clusterIndexById]);
    expect([...reversed.isolatedIds]).toEqual([...baseline.isolatedIds]);
  });

  it("excludes suggestions, self-loops and invalid weights without mutating data", () => {
    const ids = Object.freeze(["a", "b", "c", "d", "e"]);
    const edges = Object.freeze([
      { source: "a", target: "b" }, { source: "c", target: "d" },
      { source: "a", target: "c", suggested: true, weight: 1e10 },
      { source: "d", target: "e", suggested: true },
      { source: "e", target: "e", weight: 1e10 },
      ...[0, -1, Infinity, NaN].map((weight) => ({ source: "a", target: "e", weight })),
      { source: "e", target: "unknown" },
    ].map((edge) => Object.freeze(edge)));
    expect(computeGraphClusters(ids, edges)).toEqual(computeGraphClusters(ids, edges.slice(0, 2)));
    expect(computeGraphClusters(ids, edges).isolatedIds).toEqual(new Set(["e"]));
  });

  it("handles extreme positive weights without overflow or dropping real endpoints", () => {
    const ids = ["a", "b", "c", "d"];
    const result = computeGraphClusters(ids, [
      { source: "a", target: "b", weight: Number.MAX_VALUE },
      { source: "b", target: "a", weight: Number.MAX_VALUE },
      { source: "c", target: "d", weight: Number.MIN_VALUE },
    ]);
    expect(result.isolatedIds.size).toBe(0);
    expect(result.clusterIndexById.size).toBe(4);
    expect(result.clusterIndexById.get("a")).not.toBe(result.clusterIndexById.get("c"));
  });

  it("preserves relative reference weights under a common huge or tiny rescaling", () => {
    const ids = ["a", "b", "c", "d", "e", "f"];
    const edges = [
      ...clique(ids.slice(0, 3), 1000),
      ...clique(ids.slice(3), 2),
      { source: "c", target: "d", weight: 0.01 },
      { source: "b", target: "a", weight: 500 },
    ];
    const baseline = computeGraphClusters(ids, edges);
    expect(new Set(baseline.clusterIndexById.values()).size).toBe(2);
    for (const multiplier of [1e300, 1e-200]) {
      expect(computeGraphClusters(ids,
        edges.map((edge) => ({ ...edge, weight: edge.weight! * multiplier })))).toEqual(baseline);
    }
  });

  it("never returns a disconnected community on adversarial sparse graphs", () => {
    for (let seed = 1; seed <= 16; seed++) {
      const ids = Array.from({ length: 36 }, (_, i) => `n${i}`);
      let random = seed;
      const edges: GraphEdgeLike[] = [];
      for (let a = 0; a < ids.length; a++) {
        for (let b = a + 1; b < ids.length; b++) {
          random = (Math.imul(random, 1664525) + 1013904223) >>> 0;
          if (random % 19 === 0) edges.push({ source: ids[a], target: ids[b], weight: random % 7 + 1 });
        }
      }
      const { clusterIndexById } = computeGraphClusters(ids, edges);
      for (const cluster of new Set(clusterIndexById.values())) {
        const members = ids.filter((id) => clusterIndexById.get(id) === cluster);
        const reached = new Set([members[0]]);
        for (let pass = 0; pass < members.length; pass++) {
          for (const edge of edges) {
            if (clusterIndexById.get(edge.source) !== cluster ||
                clusterIndexById.get(edge.target) !== cluster) continue;
            if (reached.has(edge.source)) reached.add(edge.target);
            if (reached.has(edge.target)) reached.add(edge.source);
          }
        }
        expect(reached.size).toBe(members.length);
      }
    }
  });

  it("keeps an ambiguous complete graph together instead of inventing topic splits", () => {
    const ids = Array.from({ length: 16 }, (_, i) => `n${i}`);
    expect(new Set(computeGraphClusters(ids, clique(ids)).clusterIndexById.values()).size).toBe(1);
    expect(computeGraphClusters([], [])).toEqual({ clusterIndexById: new Map(), isolatedIds: new Set() });
  });
});

describe("cluster color palettes", () => {
  it("produces distinct colors for distinct cluster indices, stable across calls", () => {
    const palette = clusterPalette(false);
    expect(new Set(palette).size).toBe(palette.length);
    expect(clusterColor(0, false)).toBe(clusterColor(0, false));
    expect(clusterColor(0, false)).not.toBe(clusterColor(1, false));
  });

  describe("topic planet colors", () => {
    it("gives connected topic names varied, stable colors", () => {
      const topics = ["Rust", "WebAssembly", "JavaScript", "Type systems"];
      expect(new Set(topics.map(planetColor)).size).toBe(topics.length);
      expect(planetColor("Rust")).toBe(planetColor(" RUST "));
    });

    it("keeps namespace families on one hue while varying individual shades", () => {
      const colors = ["AI/Models", "AI/Research", "ai/Agents"].map(planetColor);
      expect(new Set(colors.map((color) => color.split(",")[0])).size).toBe(1);
      expect(new Set(colors).size).toBeGreaterThan(1);
      expect(planetColor("AI/Models").split(",")[0]).not.toBe(planetColor("Health/Fitness").split(",")[0]);
    });

    it("supports Unicode topic names and empty titles deterministically", () => {
      for (const title of ["数学/代数", "Café", ""]) {
        expect(planetColor(title)).toMatch(/^hsl\(\d+, \d+%, \d+%\)$/);
        expect(planetColor(title)).toBe(planetColor(title));
      }
    });
  });

  it("uses a different (darker/more saturated) palette for light themes", () => {
    const dark = clusterPalette(false);
    const light = clusterPalette(true);
    expect(dark).not.toEqual(light);
    expect(dark.length).toBe(light.length);
  });
});
