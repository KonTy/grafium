import { describe, it, expect } from "vitest";
import { renderBlock } from "./markdown";

describe("renderBlock callouts", () => {
  it("renders a tip callout with icon, title and body", () => {
    const html = renderBlock("#+BEGIN_TIP\nBe careful here.\n#+END_TIP");
    expect(html).toContain('class="callout callout-tip"');
    expect(html).toContain('class="callout-title">💡 Tip');
    expect(html).toContain("Be careful here.");
  });

  it("supports all six kinds with the right icon", () => {
    const cases: Array<[string, string, string]> = [
      ["NOTE", "note", "📝 Note"],
      ["IMPORTANT", "important", "❗ Important"],
      ["CAUTION", "caution", "⚠️ Caution"],
      ["PINNED", "pinned", "📌 Pinned"],
      ["WARNING", "warning", "🚧 Warning"],
    ];
    for (const [tag, kind, title] of cases) {
      const html = renderBlock(`#+BEGIN_${tag}\nbody\n#+END_${tag}`);
      expect(html).toContain(`class="callout callout-${kind}"`);
      expect(html).toContain(`class="callout-title">${title}`);
    }
  });

  it("is case-insensitive and renders an empty body", () => {
    const html = renderBlock("#+begin_note\n\n#+end_note");
    expect(html).toContain('class="callout callout-note"');
    expect(html).toContain('<div class="callout-body"></div>');
  });

  it("renders markdown and links inside the body", () => {
    const html = renderBlock("#+BEGIN_TIP\nSee [[Home]] and **bold**\n#+END_TIP");
    expect(html).toContain('data-page="Home"');
    expect(html).toContain("<strong>bold</strong>");
  });

  it("leaves a non-callout block untouched", () => {
    const html = renderBlock("just some text");
    expect(html).not.toContain("callout");
    expect(html).toContain("just some text");
  });

  it("does not render when begin/end kinds mismatch", () => {
    const html = renderBlock("#+BEGIN_TIP\nx\n#+END_NOTE");
    expect(html).not.toContain('class="callout');
  });
});
