import { describe, expect, it } from "vitest";
import { htmlToMarkdown, splitMarkdownIntoBlocks } from "./htmlToMd";

describe("HTML to markdown clipboard conversion", () => {
  it("preserves Grafium rendered page links", () => {
    expect(htmlToMarkdown('<a class="page-link" data-page="Tech/Android/Backup">Tech/Android/Backup</a>'))
      .toBe("[[Tech/Android/Backup]]");
  });

  it("preserves a compact concept label without changing its canonical destination", () => {
    expect(htmlToMarkdown('<a class="page-link" data-page="Insulin resistance">#insulin_resistance</a>'))
      .toBe("[[Insulin resistance|#insulin_resistance]]");
    expect(htmlToMarkdown('<a class="page-link" data-page="C++">#cpp</a>'))
      .toBe("[[C++|#cpp]]");
  });

  it("preserves non-ASCII tags through the rendered clipboard", () => {
    expect(htmlToMarkdown('<a class="tag" data-tag="睡眠/质量">#睡眠/质量</a>'))
      .toBe("#睡眠/质量");
  });

  it("preserves Grafium rendered tags and block refs", () => {
    expect(htmlToMarkdown('<a class="tag" data-tag="health/a1c">#health/a1c</a> <span class="block-ref" data-ref="abc123">((abc123))</span>'))
      .toBe("#health/a1c ((abc123))");
  });

  it("rejects spoofed Grafium page-link attributes that would inject markdown or HTML", () => {
    expect(htmlToMarkdown('<a class="page-link" data-page="Bad]]<img src=x onerror=alert(1)>[[">Visible Label</a>'))
      .toBe("Visible Label");
  });

  it("rejects spoofed task marker and fenced-code language attributes", () => {
    expect(htmlToMarkdown('<span class="task-marker" data-task-state="DONE\n<script>bad()</script>">TODO</span> item'))
      .toBe("TODO item");
    expect(htmlToMarkdown('<pre><code class="language-js`\\nalert(1)\\n```">console.log(1)</code></pre>'))
      .toBe("```\nconsole.log(1)\n```");
  });

  it("splits plain task lines into separate paste blocks", () => {
    expect(splitMarkdownIntoBlocks([
      "TODO Stocks",
      "DONE Logseq",
      "DOING GPS Tracks",
    ].join("\n"))).toEqual([
      { content: "TODO Stocks", depth: 0 },
      { content: "DONE Logseq", depth: 0 },
      { content: "DOING GPS Tracks", depth: 0 },
    ]);
  });

  it("keeps plain multiline text in one paste block", () => {
    const content = "list of motorcycles\n**Honda CRF300L**\n✅✅✅\n$5,599";
    expect(splitMarkdownIntoBlocks(content)).toEqual([{ content, depth: 0 }]);
  });

  it("splits copied outline markdown into block hierarchy", () => {
    expect(splitMarkdownIntoBlocks([
      "- [[Tech/Android/Backup]]",
      "  - TODO Stocks",
      "  - DONE Logseq",
      "    CLOSED: [2026-09-09 Wed 11:10]",
    ].join("\n"))).toEqual([
      { content: "[[Tech/Android/Backup]]", depth: 0 },
      { content: "TODO Stocks", depth: 1 },
      { content: "DONE Logseq\nCLOSED: [2026-09-09 Wed 11:10]", depth: 1 },
    ]);
  });
});
