import { describe, expect, it } from "vitest";
import type { Block } from "./api";
import {
  buildPrintDocument,
  describeMedia,
  eagerImages,
  renderPrintBlockContent,
} from "./printDocument";
import type { PrintBlock } from "./printScope";

function entry(id: string, content: string, depth = 0): PrintBlock {
  const block: Block = {
    id,
    page_id: "p",
    parent_id: null,
    order_index: 0,
    content,
    block_type: "text",
    properties: {},
    created_at: "0",
    updated_at: "0",
  };
  return { block, depth };
}

describe("eagerImages", () => {
  it("gives lazily loaded graph images a real src, so they are not blank on paper", () => {
    const html = '<img class="fc-img" loading="lazy" fetchpriority="low" data-src="asset://a.png" alt="a">';
    const printed = eagerImages(html);
    expect(printed).toContain('src="asset://a.png"');
    expect(printed).not.toContain("data-src");
    expect(printed).not.toContain("loading=");
  });

  it("leaves images that already have a src alone", () => {
    const html = '<img src="https://example.com/a.png" alt="a">';
    expect(eagerImages(html)).toBe(html);
  });
});

describe("describeMedia", () => {
  it("notes audio and video that paper cannot carry", () => {
    expect(describeMedia('<audio class="fc-audio" data-asset="talk.mp3"></audio>')).toBe(
      '<p class="print-media-note">[audio: talk.mp3]</p>',
    );
    expect(describeMedia('<video class="fc-video" data-asset="clip.mp4"></video>')).toBe(
      '<p class="print-media-note">[video: clip.mp4]</p>',
    );
  });

  it("notes embeds by their source", () => {
    expect(describeMedia('<iframe src="https://example.com/v" title="x"></iframe>')).toContain(
      "[embed: https://example.com/v]",
    );
  });
});

describe("renderPrintBlockContent", () => {
  it("renders Markdown through the same renderer the screen uses", () => {
    expect(renderPrintBlockContent("**bold**")).toContain("<strong>bold</strong>");
  });

  it("renders a heading as a heading", () => {
    expect(renderPrintBlockContent("# Chapter One")).toContain("Chapter One");
  });
});

describe("buildPrintDocument", () => {
  it("puts the title and subtitle in a header", () => {
    const html = buildPrintDocument({
      title: "My page",
      subtitle: "Grafium",
      blocks: [entry("a", "Text")],
    });
    expect(html).toContain('<h1 class="print-title">My page</h1>');
    expect(html).toContain('<p class="print-subtitle">Grafium</p>');
  });

  it("escapes the title rather than letting it become markup", () => {
    const html = buildPrintDocument({ title: '<script>x</script>', blocks: [] });
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
  });

  it("says so plainly when there is nothing to print", () => {
    expect(buildPrintDocument({ title: "Empty", blocks: [] })).toContain("print-empty");
  });

  it("skips blank blocks instead of printing stray bullets", () => {
    const html = buildPrintDocument({
      title: "T",
      blocks: [entry("a", "Real"), entry("b", "   ")],
    });
    expect(html.match(/<section class="print-block/g)).toHaveLength(1);
  });

  it("bullets an outline so its structure survives on paper", () => {
    const html = buildPrintDocument({
      title: "T",
      blocks: [entry("a", "Parent"), entry("b", "Child", 1)],
    });
    expect(html).toContain("print-block-bulleted");
    expect(html).toContain("--print-depth: 1");
  });

  it("prints a flat page as prose, with no bullets", () => {
    const html = buildPrintDocument({
      title: "Book",
      blocks: [entry("a", "First line."), entry("b", "Second line.")],
    });
    expect(html).not.toContain("print-block-bulleted");
  });

  it("marks headings so the stylesheet can keep them with their text", () => {
    const html = buildPrintDocument({ title: "T", blocks: [entry("h", "## A section")] });
    expect(html).toContain("print-block-heading print-h2");
    expect(html).not.toContain("print-block-bulleted");
  });

  it("stops indenting very deep nesting so the text column stays usable", () => {
    const html = buildPrintDocument({ title: "T", blocks: [entry("a", "Deep", 40)] });
    expect(html).toContain("--print-depth: 8");
  });
});
