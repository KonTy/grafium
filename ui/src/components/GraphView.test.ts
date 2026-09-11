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

  it("keeps hidden search misses out of picking and reports match count", () => {
    expect(source).toContain("if (!nodeMatchesSearch(node)) continue;");
    expect(source).toContain("searchMatchCount");
    expect(source).toContain('placeholder="Filter nodes…"');
    expect(source).toContain("No graph nodes match");
  });
});
