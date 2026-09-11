import { describe, it, expect } from "vitest";
import { renderBlock, renderAssistantMarkdown } from "./markdown";
import { tagColorVar } from "./tagColor";

describe("markdown tag colouring", () => {
  it("colours a #tag with its hashed accent token", () => {
    const html = renderBlock("about #recipes here");
    expect(html).toContain('class="tag"');
    expect(html).toContain('data-tag="recipes"');
    expect(html).toContain(`style="color:${tagColorVar("recipes")}"`);
  });

  it("renders hierarchical tags whole and shares the parent hue", () => {
    const urgent = renderBlock("#work/urgent");
    const later = renderBlock("#work/later");
    expect(urgent).toContain('data-tag="work/urgent"');
    expect(later).toContain('data-tag="work/later"');
    // Both derive their colour from the parent segment `work`.
    const parentVar = tagColorVar("work");
    expect(urgent).toContain(`color:${parentVar}`);
    expect(later).toContain(`color:${parentVar}`);
  });

  it("colours ((block refs)) with the distinct block-ref accent", () => {
    const html = renderBlock("see ((abc-123)) ok");
    expect(html).toContain('class="block-ref"');
    expect(html).toContain("color:var(--accent-purple)");
  });

  it("marks external http(s) links as visually distinct", () => {
    const html = renderBlock("[docs](https://example.com/page)");
    expect(html).toContain('class="external-link"');
    expect(html).toContain("color:var(--accent-cyan)");
    expect(html).toContain('href="https://example.com/page"');
  });

  it("does not mark internal/relative links as external", () => {
    const html = renderBlock("[home](some/local/page)");
    expect(html).not.toContain("external-link");
  });

  it("renders stable heading ids for in-page fragment links", () => {
    const heading = renderBlock("# THE KABALISTIC TREE OF LIFE");
    const link = renderBlock("[THE KABALISTIC TREE OF LIFE](#the-kabalistic-tree-of-life)");

    expect(heading).toContain('<h1 id="the-kabalistic-tree-of-life">');
    expect(link).toContain('href="#the-kabalistic-tree-of-life"');
    expect(link).not.toContain("external-link");
  });

  it("keeps [[page links]] on the page-link token (unchanged)", () => {
    const html = renderBlock("[[Fresco]]");
    expect(html).toContain('class="page-link"');
    expect(html).toContain('data-page="Fresco"');
    expect(html).not.toContain("external-link");
  });

  it("canonicalizes old underscore journal page links to dashed dates", () => {
    const html = renderBlock("[[2025_09_30]]");
    expect(html).toContain('class="page-link"');
    expect(html).toContain('data-page="2025-09-30"');
    expect(html).toContain(">2025-09-30</a>");
  });

  it("colours tags in assistant (chat) markdown too", () => {
    const html = renderAssistantMarkdown("Filed under #concept today.");
    expect(html).toContain('class="tag"');
    expect(html).toContain(`style="color:${tagColorVar("concept")}"`);
  });

  it("renders an immediate-complete checkbox before open task markers", () => {
    const html = renderBlock("TODO [#A] sharpen task UI");
    expect(html).toContain('class="task-checkbox unchecked todo"');
    expect(html).toContain('role="checkbox"');
    expect(html).toContain('aria-checked="false"');
    expect(html).toContain('data-task-action="done"');
    expect(html.indexOf("task-checkbox")).toBeLessThan(html.indexOf("task-marker todo"));
  });

  it("renders stripped Markdown checkbox blocks as Grafium tasks", () => {
    const html = renderBlock("[ ] Pay bills");
    expect(html).toContain('class="task-checkbox unchecked todo"');
    expect(html).toContain('class="task-marker todo"');
    expect(html).not.toContain("[ ]");
  });

  it("renders full Markdown checkbox lines as Grafium tasks", () => {
    const html = renderBlock("- [x] Beer");
    expect(html).toContain('class="task-checkbox checked done"');
    expect(html).toContain('class="task-marker done"');
    expect(html).not.toContain("[x]");
  });

  it("renders lowercase Logseq priority markers as normalized chips", () => {
    const html = renderBlock("TODO [#a] sharpen task UI");
    expect(html).toContain('class="priority priority-A"');
    expect(html).toContain(">Priority A</span>");
    expect(html).not.toContain('data-tag="A"');
    expect(html).not.toContain("[#A]");
    expect(html).not.toContain("[#a]");
  });

  it("renders completed task markers with a checked checkbox", () => {
    const html = renderBlock("DONE ship it");
    expect(html).toContain('class="task-checkbox checked done"');
    expect(html).toContain('aria-checked="true"');
    expect(html).not.toContain('data-task-action="done"');
  });

  it("hides Logseq closed/logbook metadata in rendered task blocks", () => {
    const html = renderBlock(
      [
        "DONE Logseq",
        "CLOSED: [2026-09-09 Wed 11:10]",
        ":LOGBOOK:",
        'CLOCK: [2025-09-28 Sun 09:10:03]--[2025-09-28 Sun 09:20:03] =>  00:10:00',
        '* State "DONE" from "DOING" [2026-09-09 Wed 11:10]',
        ":END:",
      ].join("\n")
    );
    expect(html).toContain('class="task-marker done"');
    expect(html).toContain("Logseq");
    expect(html).not.toContain("CLOSED:");
    expect(html).not.toContain(":LOGBOOK:");
    expect(html).not.toContain("CLOCK:");
    expect(html).not.toContain('State "DONE"');
  });

  it("keeps scheduled and deadline task badges visible", () => {
    const html = renderBlock("TODO file taxes\nSCHEDULED: <2026-01-15 Thu>\nDEADLINE: <2026-01-31 Sat>");
    expect(html).toContain('class="task-date scheduled"');
    expect(html).toContain('class="task-date deadline"');
  });
});

describe("markdown structure safety (issue #2 — no corruption)", () => {
  it("does not turn a link-destination #fragment into a tag", () => {
    const html = renderBlock("[docs](https://example.com/#section)");
    // The whole thing stays one external link, fragment preserved in href…
    expect(html).toContain('class="external-link"');
    expect(html).toContain('href="https://example.com/#section"');
    // …with no spurious tag and no leftover literal markdown.
    expect(html).not.toContain('class="tag"');
    expect(html).not.toContain("](https");
  });

  it("leaves #tags / [[links]] / ((refs)) inside ``` fenced code verbatim", () => {
    const html = renderBlock("```\n#work and [[Page]] and ((ref))\n```");
    expect(html).not.toContain('class="tag"');
    expect(html).not.toContain('class="page-link"');
    expect(html).not.toContain('class="block-ref"');
    expect(html).toContain("code-block-wrapper");
  });

  it("leaves #tags inside ~~~ fenced code verbatim", () => {
    const html = renderBlock("~~~\n#work\n~~~");
    expect(html).not.toContain('class="tag"');
    expect(html).toContain("code-block-wrapper");
  });

  it("normalizes outliner-indented closing fences before rendering", () => {
    const html = renderBlock("```bash\n#not-a-tag\n\t\t  ```\n#real-tag");
    expect(html).toContain("code-block-wrapper");
    expect(html).not.toContain('data-tag="not-a-tag"');
    expect(html).toContain('data-tag="real-tag"');
    expect(html).not.toContain("```");
  });

  it("leaves #tags inside an indented code block verbatim", () => {
    const html = renderBlock("    #work is indented code\n");
    expect(html).not.toContain('class="tag"');
  });

  it("leaves #tags inside single- and double-backtick spans verbatim", () => {
    const single = renderBlock("use `#work` inline");
    expect(single).not.toContain('class="tag"');
    expect(single).toContain("<code>#work</code>");
    const dbl = renderBlock("use ``#work`` inline");
    expect(dbl).not.toContain('class="tag"');
  });

  it("still colours a real #tag written right after a code span", () => {
    const html = renderBlock("`code` then #work");
    expect(html).toContain('data-tag="work"');
  });
});

describe("tag hierarchy canonicalization (issue #3)", () => {
  it("canonicalizes a backslash tag's nav target and hue to `/`", () => {
    const html = renderBlock("#test\\child done");
    // data-tag matches the backend's canonical `test/child` page, so a click
    // navigates to (and never creates) the right page.
    expect(html).toContain('data-tag="test/child"');
    expect(html).not.toContain("test\\child");
    // Colour is shared with the `#test` family.
    expect(html).toContain(`color:${tagColorVar("test")}`);
  });
});
