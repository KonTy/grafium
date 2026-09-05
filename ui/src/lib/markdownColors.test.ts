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

  it("keeps [[page links]] on the page-link token (unchanged)", () => {
    const html = renderBlock("[[Fresco]]");
    expect(html).toContain('class="page-link"');
    expect(html).toContain('data-page="Fresco"');
    expect(html).not.toContain("external-link");
  });

  it("colours tags in assistant (chat) markdown too", () => {
    const html = renderAssistantMarkdown("Filed under #concept today.");
    expect(html).toContain('class="tag"');
    expect(html).toContain(`style="color:${tagColorVar("concept")}"`);
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
