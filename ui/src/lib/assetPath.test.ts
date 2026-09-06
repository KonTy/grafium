import { describe, it, expect, afterEach } from "vitest";
import { renderBlock, setAssetBaseDir } from "./markdown";

afterEach(() => setAssetBaseDir(""));

/**
 * Two reference shapes resolve differently, deliberately. The historical
 * `../assets/…` form always meant "from the graph root" whatever the page's
 * depth, and thousands of existing references rely on that. A plain relative
 * path resolves against the page's own directory instead — the way every other
 * markdown tool reads it — which is what lets a book's media live beside the
 * book and survive the folder being copied elsewhere.
 */
describe("asset path resolution", () => {
  it("keeps legacy ../assets references pointing at the graph root", () => {
    setAssetBaseDir("pages/mybooks/coolbook");
    const html = renderBlock("![cover](../assets/tutorial/x.svg)");
    expect(html).toContain("assets/tutorial/x.svg");
    expect(html).not.toContain("pages/mybooks/coolbook/assets/tutorial/x.svg");
  });

  it("resolves a plain relative path against the page's own folder", () => {
    setAssetBaseDir("pages/mybooks/coolbook");
    const html = renderBlock("![cover](assets/cover.png)");
    expect(html).toContain("pages/mybooks/coolbook/assets/cover.png");
  });

  it("falls back to the graph root when no page context is set", () => {
    setAssetBaseDir("");
    const html = renderBlock("![cover](assets/cover.png)");
    expect(html).toContain("assets/cover.png");
    expect(html).not.toContain("pages/");
  });

  it("still rejects traversal out of the graph", () => {
    setAssetBaseDir("pages/deep");
    const html = renderBlock("![x](../../../../etc/passwd)");
    expect(html).not.toContain("..");
  });
});
