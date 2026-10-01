import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");

describe("native asset cleanup warnings", () => {
  it("subscribes to graph-bound warnings and shows the native message as an error", () => {
    const start = source.indexOf('listen<{ graphPath: string; message: string }>("asset-cleanup-warning"');
    expect(start).toBeGreaterThan(-1);
    const listener = source.slice(start, source.indexOf('"graph-sources-changed"', start));
    expect(listener).toContain("if (disposed) return;");
    expect(listener).toContain("const graph = await getGraphInfo();");
    expect(listener).toContain("if (!disposed && graph.path === payload.graphPath)");
    expect(listener).toContain('showToast(payload.message, "error");');
    expect(listener).toContain("Could not inspect the graph after an asset cleanup warning:");
  });

  it("shares the managed subscription lifecycle with native source-index events", () => {
    const start = source.indexOf("const subscriptions = [");
    const end = source.indexOf("stops.forEach(stop => stop())", start);
    const subscriptions = source.slice(start, end);
    expect(subscriptions).toContain('"asset-cleanup-warning"');
    expect(subscriptions).toContain("Promise.allSettled(subscriptions)");
    expect(subscriptions).toContain("if (disposed) result.value();");
    expect(subscriptions).toContain("else stops.push(result.value);");
    expect(subscriptions).toContain("disposed = true;");
  });
});
