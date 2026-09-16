import { describe, expect, it } from "vitest";
import source from "./GraphView.svelte?raw";

describe("GraphView search", () => {
  it("uses fuzzy search to filter visible 2D graph nodes", () => {
    expect(source).toContain('import { fuzzyScore } from "../lib/fuzzy"');
    expect(source).toContain("function nodeMatchesSearch(node: SimNode)");
    expect(source).toContain("function visibleNodeIdSet()");
    expect(source).toContain("if (visibleIds && !visibleIds.has(node.id)) continue;");
    expect(source).toContain("if (visibleIds && (!visibleIds.has(e.source.id) || !visibleIds.has(e.target.id)))");
  });

  describe("GraphView community layout", () => {
    it("uses the shared weighted layout instead of randomized component positions", () => {
      expect(source).toContain("createCommunityLayout(data.nodes, data.edges, 2)");
      expect(source).toContain("communityLinkDistance(layout, e.source, e.target)");
      expect(source).toContain('import { clusterColor } from "../lib/graphClusters"');
      expect(source).not.toContain("Math.random()");
    });

    it("anchors communities, leaves suggestions out of physics, and fits actual bounds", () => {
      expect(source).toContain("node.anchorX * layoutScale - node.x");
      expect(source).toContain("if (e.suggested) continue;");
      expect(source).toContain("node.x - radiusOf(node)");
      expect(source).toContain("onMount(() =>");
    });
  });

  it("keeps hidden search misses out of picking and reports match count", () => {
    expect(source).toContain("if (!nodeMatchesSearch(node)) continue;");
    expect(source).toContain("searchMatchCount");
    expect(source).toContain('placeholder="Filter nodes…"');
    expect(source).toContain("No graph nodes match");
  });
});
