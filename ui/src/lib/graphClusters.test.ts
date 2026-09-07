import { describe, expect, it } from "vitest";
import { computeGraphClusters, clusterColor, clusterPalette } from "./graphClusters";

describe("computeGraphClusters", () => {
  it("groups connected nodes into the same cluster and orders clusters by size", () => {
    // Cluster A: a-1..a-4 (a chain), Cluster B: b-1, b-2, isolated: c-1
    const nodeIds = ["a-1", "a-2", "a-3", "a-4", "b-1", "b-2", "c-1"];
    const edges = [
      { source: "a-1", target: "a-2" },
      { source: "a-2", target: "a-3" },
      { source: "a-3", target: "a-4" },
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
});

describe("cluster color palettes", () => {
  it("produces distinct colors for distinct cluster indices, stable across calls", () => {
    const palette = clusterPalette(false);
    expect(new Set(palette).size).toBe(palette.length);
    expect(clusterColor(0, false)).toBe(clusterColor(0, false));
    expect(clusterColor(0, false)).not.toBe(clusterColor(1, false));
  });

  it("uses a different (darker/more saturated) palette for light themes", () => {
    const dark = clusterPalette(false);
    const light = clusterPalette(true);
    expect(dark).not.toEqual(light);
    expect(dark.length).toBe(light.length);
  });
});
