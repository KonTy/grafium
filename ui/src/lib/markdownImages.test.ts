import { describe, expect, it } from "vitest";
import {
  clearMarkdownImageWidth,
  hydrateAssetMedia,
  renderBlock,
  setMarkdownImageWidth,
} from "./markdown";

describe("markdown image rendering", () => {
  it("defers graph-local image src assignment", () => {
    const host = document.createElement("div");
    host.innerHTML = renderBlock("![Figure](assets/page-2-figure.png)", "pages/Books/Book");
    const img = host.querySelector("img.fc-img") as HTMLImageElement | null;

    expect(img).not.toBeNull();
    expect(img?.getAttribute("src")).toBeNull();
    expect(img?.getAttribute("data-src")).toContain(
      "grafium-asset://localhost/pages/Books/Book/assets/page-2-figure.png"
    );
    expect(img?.getAttribute("decoding")).toBe("async");
  });

  it("keeps remote images as normal lazy images", () => {
    const host = document.createElement("div");
    host.innerHTML = renderBlock("![Remote](https://example.com/image.png)");
    const img = host.querySelector("img.fc-img") as HTMLImageElement | null;

    expect(img?.getAttribute("src")).toBe("https://example.com/image.png");
    expect(img?.getAttribute("data-src")).toBeNull();
  });

  it("hydrates local images without IntersectionObserver support", () => {
    const host = document.createElement("div");
    host.innerHTML = renderBlock("![Figure](assets/page-2-figure.png)", "pages/Books/Book");
    const img = host.querySelector("img.fc-img") as HTMLImageElement;

    const originalIntersectionObserver = globalThis.IntersectionObserver;
    // @ts-expect-error Simulate older webviews for the fallback branch.
    delete globalThis.IntersectionObserver;
    try {
      const cleanup = hydrateAssetMedia(host);
      expect(img.getAttribute("src")).toContain(
        "grafium-asset://localhost/pages/Books/Book/assets/page-2-figure.png"
      );
      cleanup();
    } finally {
      globalThis.IntersectionObserver = originalIntersectionObserver;
    }
  });

  it("renders persisted image widths from markdown attributes", () => {
    const host = document.createElement("div");
    host.innerHTML = renderBlock(
      "![Figure](assets/page-2-figure.png){:width 640, :height 480}",
      "pages/Books/Book"
    );
    const img = host.querySelector("img.fc-img") as HTMLImageElement | null;

    expect(img?.getAttribute("style")).toContain("width: 640px");
    expect(img?.getAttribute("style")).toContain("height: auto");
    expect(img?.getAttribute("style")).not.toContain("height: 480px");
    expect(img?.getAttribute("data-image-width")).toBe("640");
    expect(img?.getAttribute("data-image-height")).toBe("480");
    expect(img?.getAttribute("data-src")).toContain("page-2-figure.png");
    expect(host.textContent).not.toContain("{:width");
  });

  it("still reads Logseq-style image attributes with height present", () => {
    const host = document.createElement("div");
    host.innerHTML = renderBlock("![Figure](assets/page-2-figure.png){:height 76, :width 341}", "pages/Books/Book");
    const img = host.querySelector("img.fc-img") as HTMLImageElement | null;

    expect(img?.getAttribute("style")).toContain("width: 341px");
    expect(img?.getAttribute("style")).toContain("height: auto");
    expect(img?.getAttribute("style")).not.toContain("height: 76px");
    expect(img?.getAttribute("data-image-width")).toBe("341");
    expect(img?.getAttribute("data-image-height")).toBe("76");
    expect(host.textContent).not.toContain("{:width");
  });

  it("still reads pipe image sizes without treating them as part of the path", () => {
    const host = document.createElement("div");
    host.innerHTML = renderBlock("![Figure](assets/page-2-figure.png|512x384)", "pages/Books/Book");
    const img = host.querySelector("img.fc-img") as HTMLImageElement | null;

    expect(img?.getAttribute("style")).toContain("width: 512px");
    expect(img?.getAttribute("style")).toContain("height: auto");
    expect(img?.getAttribute("style")).not.toContain("height: 384px");
    expect(img?.getAttribute("data-src")).toContain("page-2-figure.png");
    expect(img?.getAttribute("data-src")).not.toContain("|512x384");
  });

  it("updates the selected markdown image width without touching code examples", () => {
    const content = [
      "![One](assets/one.png)",
      "`![Ignored](assets/ignored.png)`",
      "![Two](assets/two.png){:width 200}",
    ].join("\n");

    expect(setMarkdownImageWidth(content, 1, 320)).toBe([
      "![One](assets/one.png)",
      "`![Ignored](assets/ignored.png)`",
      "![Two](assets/two.png){:width 320}",
    ].join("\n"));
  });

  it("writes width-only image attributes when resizing", () => {
    expect(setMarkdownImageWidth(
      "![Figure](assets/figure.png){:width 300, :height 120}",
      0,
      600,
    )).toBe("![Figure](assets/figure.png){:width 600}");
  });

  it("clears persisted image widths without touching other images", () => {
    expect(clearMarkdownImageWidth(
      "![One](assets/one.png|240)\n![Two](assets/two.png){:width 480}",
      0,
    )).toBe("![One](assets/one.png)\n![Two](assets/two.png){:width 480}");
  });
});
