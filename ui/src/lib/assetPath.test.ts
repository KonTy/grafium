import { describe, it, expect } from "vitest";
import { renderBlock, assetBaseDirFor } from "./markdown";

const BOOK = "pages/mybooks/coolbook";

/**
 * Two reference shapes resolve differently, deliberately. The historical
 * `../assets/…` form always meant "from the graph root" whatever the page's
 * depth, and thousands of existing references rely on that. A plain relative
 * path resolves against the page's own directory instead — the way every other
 * markdown tool reads it — which is what lets a book's media live beside the
 * book and survive the folder being copied elsewhere.
 *
 * The base directory is an argument rather than ambient state on purpose: the
 * journal view renders several pages at once, so there is no single "current
 * page" for a render to read from.
 */
describe("asset path resolution", () => {
  it("keeps legacy ../assets references pointing at the graph root", () => {
    const html = renderBlock("![cover](../assets/tutorial/x.svg)", BOOK);
    expect(html).toContain("assets/tutorial/x.svg");
    expect(html).not.toContain(`${BOOK}/assets/tutorial/x.svg`);
  });

  it("keeps deeper ../../ references at the graph root too", () => {
    const html = renderBlock("![cover](../../assets/x.png)", BOOK);
    expect(html).toContain("localhost/assets/x.png");
  });

  it("resolves a plain relative path against the page's own folder", () => {
    const html = renderBlock("![cover](assets/cover.png)", BOOK);
    expect(html).toContain(`${BOOK}/assets/cover.png`);
  });

  it("treats ./assets the same as plain assets", () => {
    // `./x` and `x` mean the same thing in every markdown tool. Classifying
    // the dot-slash form as root-relative made them resolve to different files.
    const plain = renderBlock("![a](assets/cover.png)", BOOK);
    const dotted = renderBlock("![a](./assets/cover.png)", BOOK);
    expect(dotted).toContain(`${BOOK}/assets/cover.png`);
    expect(dotted).toBe(plain);
  });

  it("falls back to the graph root when no page context is given", () => {
    const html = renderBlock("![cover](assets/cover.png)");
    expect(html).toContain("assets/cover.png");
    expect(html).not.toContain("pages/");
  });

  it("still rejects traversal out of the graph", () => {
    const html = renderBlock("![x](../../../../etc/passwd)", "pages/deep");
    expect(html).not.toContain("..");
  });

  it("renders the same block differently for different pages", () => {
    // The rendered-block cache was keyed on content alone, which silently
    // served one page's asset URLs on another.
    const block = "![cover](assets/cover.png)";
    expect(renderBlock(block, "pages/one")).toContain("pages/one/assets/cover.png");
    expect(renderBlock(block, "pages/two")).toContain("pages/two/assets/cover.png");
  });

  it("does not leak a page's directory into a later rootless render", () => {
    renderBlock("![a](assets/a.png)", BOOK);
    const rootless = renderBlock("![b](assets/b.png)");
    expect(rootless).not.toContain(BOOK);
  });

  it("leaves absolute and data URLs alone", () => {
    expect(renderBlock("![a](https://example.com/x.png)", BOOK)).toContain(
      "https://example.com/x.png",
    );
    expect(renderBlock("![a](data:image/png;base64,AAA)", BOOK)).toContain("data:image/png");
  });

  it("handles spaces and unicode in a page-relative reference", () => {
    const html = renderBlock("![a](<assets/photo 1.png>)", BOOK);
    expect(html).toContain(`${BOOK}/assets/photo%201.png`);
    expect(renderBlock("![a](assets/写真.png)", BOOK)).toContain(
      `${BOOK}/assets/${encodeURIComponent("写真.png")}`,
    );
  });

  it("escapes # and ? in a page directory instead of ending the URL path", () => {
    // `encodeURI` leaves both alone because they are legal URL syntax, so a
    // page at `pages/C#/intro.md` produced a path that stopped at `pages/C`.
    const html = renderBlock("![a](assets/x.png)", "pages/C#/intro");
    expect(html).toContain("pages/C%23/intro/assets/x.png");
    expect(renderBlock("![a](assets/x.png)", "pages/why?/intro")).toContain(
      "pages/why%3F/intro/assets/x.png",
    );
  });
});

describe("assetBaseDirFor", () => {
  it("returns the directory a page's markdown file sits in", () => {
    expect(assetBaseDirFor("pages/mybooks/coolbook/toc.md")).toBe(BOOK);
    expect(assetBaseDirFor("journals/2025_01_15.md")).toBe("journals");
  });

  it("treats a page with no file yet as having no directory", () => {
    // Linking to a page creates it in the database with no file on disk.
    expect(assetBaseDirFor(null)).toBe("");
    expect(assetBaseDirFor(undefined)).toBe("");
    expect(assetBaseDirFor("")).toBe("");
  });

  it("handles a top-level file and windows separators", () => {
    expect(assetBaseDirFor("toc.md")).toBe("");
    expect(assetBaseDirFor("pages\\mybooks\\toc.md")).toBe("pages/mybooks");
  });
});
