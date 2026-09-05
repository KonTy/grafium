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
