import { describe, it, expect } from "vitest";
import { renderAssistantMarkdown } from "./markdown";

describe("renderAssistantMarkdown", () => {
  it("renders common markdown instead of raw symbols", () => {
    const html = renderAssistantMarkdown(
      "**bold** and _italic_\n\n## Heading\n\n- one\n- two\n\n```js\nconst x = 1;\n```"
    );
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toMatch(/<h2[^>]*>Heading<\/h2>/);
    expect(html).toContain("<li>one</li>");
    expect(html).toContain("code-block-wrapper");
    // No raw markdown symbols left over for the common constructs.
    expect(html).not.toContain("**bold**");
    expect(html).not.toContain("## Heading");
  });

  it("turns [[wiki links]] into clickable page-link anchors", () => {
    const html = renderAssistantMarkdown("See [[Fresco]] for details.");
    expect(html).toContain('class="page-link"');
    expect(html).toContain('data-page="Fresco"');
  });

  it("turns #tags into clickable tag anchors", () => {
    const html = renderAssistantMarkdown("Filed under #concept today.");
    expect(html).toContain('class="tag"');
    expect(html).toContain('data-tag="concept"');
  });

  it("neutralizes a <script> payload", () => {
    const html = renderAssistantMarkdown(
      "Hello <script>alert('xss')</script> world"
    );
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("</script>");
    expect(html).toContain("&lt;script");
  });

  it("neutralizes an <img onerror> payload", () => {
    const html = renderAssistantMarkdown('<img src=x onerror="alert(1)">');
    expect(html).not.toMatch(/<img[^>]*onerror/i);
    expect(html).toContain("&lt;img");
  });

  it("strips a javascript: link href", () => {
    const html = renderAssistantMarkdown("[click me](javascript:alert(1))");
    expect(html).not.toContain("javascript:");
    // The link text still renders; only the dangerous scheme is removed.
    expect(html).toContain("click me");
  });

  it("keeps safe external links intact", () => {
    const html = renderAssistantMarkdown("[docs](https://example.com/path)");
    expect(html).toContain('href="https://example.com/path"');
  });

  it("does not throw on truncated markdown (unclosed fence)", () => {
    expect(() =>
      renderAssistantMarkdown("Here is code:\n\n```js\nconst x = ")
    ).not.toThrow();
  });

  it("does not throw on a half-written wiki link", () => {
    expect(() => renderAssistantMarkdown("Look at [[Half writt")).not.toThrow();
  });

  it("leaves HTML-looking text inside code spans verbatim (escaped, not executed)", () => {
    const html = renderAssistantMarkdown("Use `<script>` carefully");
    // Inside inline code marked escapes it; either way it must not be a live tag.
    expect(html).not.toContain("<script>");
    expect(html).toContain("<code>");
  });
});
