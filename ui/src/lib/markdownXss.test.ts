import { describe, it, expect } from "vitest";
import { renderAssistantMarkdown } from "./markdown";

/**
 * Assistant answers are untrusted: they are a local model's output, and since
 * web research landed they routinely quote text fetched from arbitrary sites.
 * The rendered HTML goes straight into `{@html}` and the app sets no CSP, so
 * anything that survives here executes.
 */
describe("assistant markdown is safe to inject", () => {
  it("does not let an image URL close its own src attribute", () => {
    // Regression: the URL was interpolated into `src` unescaped, so this
    // rendered as `<img src="https://h/x"onerror="alert(1)" …>`.
    const html = renderAssistantMarkdown('![x](https://h.invalid/x"onerror="alert(1))');
    expect(html).not.toMatch(/onerror/i);
  });

  it("strips raw HTML that survives a mismatched code fence", () => {
    // The source-level escaper decides "inside a code span" with a regex while
    // marked decides with a parser; any disagreement used to yield live markup.
    const host = document.createElement("div");
    host.innerHTML = renderAssistantMarkdown("```\n<svg onload=alert(1)>\n````");
    // Rendered as code text is fine; rendered as a live element is not.
    expect(host.querySelector("svg")).toBeNull();
    for (const el of Array.from(host.querySelectorAll("*"))) {
      for (const attr of Array.from(el.attributes)) {
        expect(attr.name.toLowerCase().startsWith("on")).toBe(false);
      }
    }
  });

  it("removes dangerous elements and handlers however they are introduced", () => {
    for (const input of [
      "<script>alert(1)</script>",
      "<img src=x onerror=alert(1)>",
      "<iframe src=https://evil.invalid></iframe>",
      '<a href="javascript:alert(1)">click</a>',
      '<div onmouseover="alert(1)">hover</div>',
      '<svg><animate onbegin="alert(1)" /></svg>',
    ]) {
      // Assert against the parsed DOM, not the string: escaped text like
      // `&lt;img onerror=…&gt;` is inert, and string matching would flag it as
      // a vulnerability while missing a genuinely live attribute.
      const host = document.createElement("div");
      host.innerHTML = renderAssistantMarkdown(input);

      expect(host.querySelector("script")).toBeNull();
      expect(host.querySelector("iframe")).toBeNull();
      for (const el of Array.from(host.querySelectorAll("*"))) {
        for (const attr of Array.from(el.attributes)) {
          expect(attr.name.toLowerCase().startsWith("on")).toBe(false);
          expect(attr.value.toLowerCase()).not.toContain("javascript:");
        }
      }
    }
  });

  it("keeps ordinary formatting intact", () => {
    const html = renderAssistantMarkdown(
      "# Title\n\n**bold** and `code`\n\n- item\n\n[link](https://example.com)"
    );
    expect(html).toContain("<strong>");
    expect(html).toContain("<code");
    expect(html).toContain("<li>");
    expect(html).toContain('href="https://example.com"');
  });
});
