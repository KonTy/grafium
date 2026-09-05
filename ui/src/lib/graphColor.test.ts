import { describe, it, expect } from "vitest";
import { assignClusterHues, edgeHue, exceedsDragThreshold } from "./graphColor";
import { TAG_HUES } from "./tagColor";

describe("assignClusterHues", () => {
  it("gives every node in a connected component the same hue", () => {
    const hues = assignClusterHues(
      ["a", "b", "c"],
      [
        { source: "a", target: "b" },
        { source: "b", target: "c" },
      ]
    );
    expect(hues.get("a")).toBe(hues.get("b"));
    expect(hues.get("b")).toBe(hues.get("c"));
  });

  it("gives separate clusters different hues", () => {
    const hues = assignClusterHues(
      ["a", "b", "x", "y"],
      [
        { source: "a", target: "b" },
        { source: "x", target: "y" },
      ]
    );
    expect(hues.get("a")).not.toBe(hues.get("x"));
  });

  /// The largest cluster must keep its hue as the layout settles, or the graph
  /// would visibly re-colour itself while the simulation runs.
  it("is deterministic and ranks the biggest cluster first", () => {
    const nodes = ["a", "b", "c", "x", "y"];
    const edges = [
      { source: "a", target: "b" },
      { source: "b", target: "c" },
      { source: "x", target: "y" },
    ];
    const first = assignClusterHues(nodes, edges);
    const second = assignClusterHues([...nodes].reverse(), edges);
    expect(first.get("a")).toBe(TAG_HUES[0]);
    expect(second.get("a")).toBe(TAG_HUES[0]);
    expect(first.get("x")).toBe(second.get("x"));
  });

  /// Isolated nodes carry no grouping information; letting each take a hue
  /// would exhaust an 8-colour palette and leave real clusters sharing.
  it("collapses isolated nodes onto a single hue", () => {
    const hues = assignClusterHues(["l1", "l2", "l3"], []);
    expect(hues.get("l1")).toBe(hues.get("l2"));
    expect(hues.get("l2")).toBe(hues.get("l3"));
  });

  it("ignores edges referencing nodes outside the view", () => {
    const hues = assignClusterHues(["a"], [{ source: "a", target: "offscreen" }]);
    expect(hues.get("a")).toBeDefined();
    expect(hues.has("offscreen")).toBe(false);
  });

  it("assigns a hue to every node", () => {
    const nodes = Array.from({ length: 40 }, (_, i) => `n${i}`);
    const edges = nodes.slice(1).map((n, i) => ({ source: nodes[i], target: n }));
    const hues = assignClusterHues(nodes, edges);
    for (const n of nodes) expect(hues.get(n)).toBeDefined();
  });
});

describe("edgeHue", () => {
  it("colours an edge inside a cluster and leaves bridges neutral", () => {
    const hues = assignClusterHues(
      ["a", "b", "x", "y"],
      [
        { source: "a", target: "b" },
        { source: "x", target: "y" },
      ]
    );
    expect(edgeHue(hues, { source: "a", target: "b" })).toBe(hues.get("a"));
    // A bridge belongs to neither cluster, so it has no owning hue.
    expect(edgeHue(hues, { source: "a", target: "x" })).toBeNull();
  });
});

describe("exceedsDragThreshold", () => {
  /// Regression: graph nodes were unclickable because any pointermove at all
  /// marked the gesture a drag, and a real mouse jitters between press and
  /// release on nearly every click.
  it("treats sub-threshold jitter as a click", () => {
    expect(exceedsDragThreshold(100, 100, 100, 100)).toBe(false);
    expect(exceedsDragThreshold(100, 100, 101, 101)).toBe(false);
    expect(exceedsDragThreshold(100, 100, 102, 2 + 100)).toBe(false);
  });

  it("treats real movement as a drag", () => {
    expect(exceedsDragThreshold(100, 100, 120, 100)).toBe(true);
    expect(exceedsDragThreshold(100, 100, 100, 140)).toBe(true);
  });

  it("is direction-agnostic", () => {
    expect(exceedsDragThreshold(100, 100, 80, 100)).toBe(true);
    expect(exceedsDragThreshold(100, 100, 100, 60)).toBe(true);
  });
});
