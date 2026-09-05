import { describe, it, expect } from "vitest";
import { highlightTerm, clearHighlights, HIGHLIGHT_CLASS } from "./highlight";

function host(html: string): HTMLElement {
  const el = document.createElement("div");
  el.innerHTML = html;
  return el;
}

describe("highlightTerm", () => {
  it("wraps every match and returns the first", () => {
    const el = host("<p>alpha beta alpha</p>");
    const first = highlightTerm(el, "alpha");
    expect(el.querySelectorAll(`mark.${HIGHLIGHT_CLASS}`).length).toBe(2);
    expect(first?.textContent).toBe("alpha");
  });

  it("matches case-insensitively but preserves the page's capitalisation", () => {
    const el = host("<p>Creatine and CREATINE</p>");
    highlightTerm(el, "creatine");
    const marks = Array.from(el.querySelectorAll("mark")).map((m) => m.textContent);
    expect(marks).toEqual(["Creatine", "CREATINE"]);
  });

  /// Rewriting the HTML string instead of the DOM would match inside `href`
  /// and destroy the link — the reason this walks text nodes.
  it("never touches attributes or link structure", () => {
    const el = host('<p><a href="https://example.com/alpha">alpha</a></p>');
    highlightTerm(el, "alpha");
    const link = el.querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://example.com/alpha");
    expect(link?.querySelector("mark")).not.toBeNull();
  });

  it("leaves code spans and blocks alone", () => {
    const el = host("<p><code>alpha</code> alpha</p>");
    highlightTerm(el, "alpha");
    expect(el.querySelector("code mark")).toBeNull();
    expect(el.querySelectorAll("mark").length).toBe(1);
  });

  it("returns null when there is nothing to highlight", () => {
    expect(highlightTerm(host("<p>nothing here</p>"), "absent")).toBeNull();
    expect(highlightTerm(host("<p>text</p>"), "   ")).toBeNull();
  });

  /// Repeated navigation must not nest marks or leave stale ones behind.
  it("replaces previous highlights rather than stacking them", () => {
    const el = host("<p>alpha beta</p>");
    highlightTerm(el, "alpha");
    highlightTerm(el, "beta");
    expect(el.querySelectorAll("mark").length).toBe(1);
    expect(el.querySelector("mark")?.textContent).toBe("beta");
    expect(el.textContent).toBe("alpha beta");
  });
});

describe("clearHighlights", () => {
  it("restores the original text exactly", () => {
    const el = host("<p>alpha beta alpha</p>");
    const before = el.textContent;
    highlightTerm(el, "alpha");
    clearHighlights(el);
    expect(el.querySelectorAll("mark").length).toBe(0);
    expect(el.textContent).toBe(before);
  });

  /// Unwrapping leaves adjacent text nodes; without normalize() a paragraph
  /// would shatter into fragments over repeated cycles.
  it("merges the text nodes it leaves behind", () => {
    const el = host("<p>alpha beta alpha</p>");
    for (let i = 0; i < 5; i++) {
      highlightTerm(el, "alpha");
      clearHighlights(el);
    }
    expect(el.querySelector("p")?.childNodes.length).toBe(1);
  });
});
