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

  it("preserves a web table as a Markdown table", () => {
    const html = `<div class="c+qhpcvP6H3VCSU9BgHRlQ==">
      <table class="zxqV+AyRUca+MLhV37xZ3A==">
        <thead class="vws+0UuF+kO6CYMvFHvtZQ==">
          <tr class="_2ebVTgiBfkHr-I+WBkJGUw==">
            <th class="Twce1OmS+ZcG-oo8+adRMg==">Item</th>
            <th class="Twce1OmS+ZcG-oo8+adRMg==">Amount</th>
          </tr>
        </thead>
        <tbody class="o4qKFCTY3cX9YjCR2ocmKA==">
          <tr class="_2ebVTgiBfkHr-I+WBkJGUw==">
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Item"><strong>Withdraw at ATM</strong></td>
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Amount"><strong>¥45,000 (~$290)</strong></td>
          </tr>
          <tr class="_2ebVTgiBfkHr-I+WBkJGUw==">
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Item">Load onto Suica now</td>
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Amount">¥5,000</td>
          </tr>
          <tr class="_2ebVTgiBfkHr-I+WBkJGUw==">
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Item">Hakone Free Pass (buy Sept 23 at Shinjuku)</td>
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Amount">~¥6,100 cash/card</td>
          </tr>
          <tr class="_2ebVTgiBfkHr-I+WBkJGUw==">
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Item">Hakone round-trip Shinjuku↔Hakone</td>
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Amount">~¥2,600 (Suica)</td>
          </tr>
          <tr class="_2ebVTgiBfkHr-I+WBkJGUw==">
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Item">Tokyo local trains, Sept 21–27</td>
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Amount">~¥3,000 (top up as needed)</td>
          </tr>
          <tr class="_2ebVTgiBfkHr-I+WBkJGUw==">
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Item">Food, temples, small shops, vending machines</td>
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Amount">¥20,000+ cash</td>
          </tr>
          <tr class="_2ebVTgiBfkHr-I+WBkJGUw==">
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Item">Buffer</td>
            <td class="Lj1YMIu6HckHEepSU13LHg==" data-label="Amount">¥8,000</td>
          </tr>
        </tbody>
      </table>
    </div>`;

    const markdown = [
      "| Item | Amount |",
      "| --- | --- |",
      "| **Withdraw at ATM** | **¥45,000 (~$290)** |",
      "| Load onto Suica now | ¥5,000 |",
      "| Hakone Free Pass (buy Sept 23 at Shinjuku) | ~¥6,100 cash/card |",
      "| Hakone round-trip Shinjuku↔Hakone | ~¥2,600 (Suica) |",
      "| Tokyo local trains, Sept 21–27 | ~¥3,000 (top up as needed) |",
      "| Food, temples, small shops, vending machines | ¥20,000+ cash |",
      "| Buffer | ¥8,000 |",
    ].join("\n");

    expect(htmlToMarkdown(html)).toBe(markdown);
    expect(splitMarkdownIntoBlocks(markdown)).toEqual([
      {
        content: markdown,
        depth: 0,
      },
    ]);
  });

  it("escapes cell pipes and preserves inline formatting and line breaks", () => {
    const html = [
      "<table><tbody>",
      "<tr><td>Option</td><td>Details</td></tr>",
      "<tr><td><strong>A | B</strong></td><td>First<br>Second</td></tr>",
      "</tbody></table>",
    ].join("");

    expect(htmlToMarkdown(html)).toBe([
      "| Option | Details |",
      "| --- | --- |",
      "| **A \\| B** | First<br>Second |",
    ].join("\n"));
  });

  it("preserves standards-based ARIA grids and column spans", () => {
    const html = [
      '<div role="grid">',
      '<div role="row"><span role="columnheader">Item</span><span role="columnheader">Amount</span></div>',
      '<div role="row"><span role="gridcell" aria-colspan="2"><strong>Total: ¥53,000</strong></span></div>',
      "</div>",
    ].join("");

    expect(htmlToMarkdown(html)).toBe([
      "| Item | Amount |",
      "| --- | --- |",
      "| **Total: ¥53,000** |  |",
    ].join("\n"));
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
