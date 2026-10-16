import { describe, expect, it } from "vitest";
import { outlineListMarker, renderAssistantMarkdown, renderBlock } from "./markdown";

describe("distinct Markdown list markers", () => {
  it.each([
    ["-", "diamond", "\u25c6"],
    ["+", "square", "\u25a0"],
    ["*", "dot", "\u2022"],
  ])("renders %s lists with the %s marker", (source, shape, symbol) => {
    const html = renderBlock(`${source} First\n${source} Second`);
    expect(html).toContain(`<ul class="grafium-list-${shape}">`);
    expect(outlineListMarker(html)).toEqual({ shape, symbol });
    expect(renderAssistantMarkdown(`${source} First`)).toContain(`grafium-list-${shape}`);
  });

  it("keeps each nested or adjacent list's marker", () => {
    const root = document.createElement("div");
    root.innerHTML = renderBlock("- Diamond\n  + Square\n    * Dot\n\n+ Separate");
    expect([...root.querySelectorAll("ul")].map((list) => list.className)).toEqual([
      "grafium-list-diamond", "grafium-list-square", "grafium-list-dot", "grafium-list-square",
    ]);
  });

  it.each(["Plain", "\\- Literal", "---", "***", "**Bold**", "_ text", "`- Code`",
    "```\n- Code\n```", "    - Indented code", "Paragraph\n\n- Later list",
    "> - Quoted list", "- [ ] Task"])("does not mistake %s for a leading bullet", (source) => {
    expect(outlineListMarker(renderBlock(source)).shape).toBe("dot");
  });

  it("preserves ordered list numbering", () => {
    const html = renderBlock("3. Third\n4. Fourth");
    expect(html).toContain('<ol start="3">');
    expect(html).not.toContain("grafium-list-");
    expect(outlineListMarker(html)).toEqual({ shape: "ordered", symbol: "" });
  });

  it.each(["2.1.", "12.10.3)"])("renders hierarchical %s exactly once with inline formatting", (marker) => {
    const root = document.createElement("div");
    root.innerHTML = renderBlock(`${marker} **Step** [[Guide]]`);
    expect(root.querySelector(".ordered-list-label")?.textContent).toBe(marker);
    expect(root.querySelectorAll("ol > li")).toHaveLength(1);
    expect(root.querySelector("strong")?.textContent).toBe("Step");
    expect(root.querySelector(".page-link")?.textContent).toBe("Guide");
    expect(outlineListMarker(root.innerHTML).shape).toBe("ordered");
    expect(renderAssistantMarkdown(`${marker} Step`)).toContain("ordered-list-label");
  });

  it.each(["1.5 metres", "`2.1. Literal`", "```\n2.1. Literal\n```", "    2.1. Code"])(
    "does not render prose or code %s as a hierarchical list", (source) => {
      expect(renderBlock(source)).not.toContain("ordered-list-label");
    },
  );
});
