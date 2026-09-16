import { describe, expect, it } from "vitest";
import { formatConceptLink, formatConceptTag } from "./conceptLinks";
import { renderAssistantMarkdown, renderBlock } from "./markdown";

function rendered(source: string): HTMLDivElement {
  const node = document.createElement("div");
  node.innerHTML = renderBlock(source);
  return node;
}

describe("wiki targets and display aliases", () => {
  it("keeps the canonical page separate from generated concept display", () => {
    const node = rendered("[[Insulin resistance|#insulin_resistance]]");
    const link = node.querySelector("a")!;
    expect(link.dataset.page).toBe("Insulin resistance");
    expect(link.textContent).toBe("#insulin_resistance");
    expect(link.className).toBe("page-link");
    expect(node.querySelector("[data-tag]")).toBeNull();
  });

  it("preserves manual display, including punctuation and extra pipes", () => {
    const node = rendered("[[ A\\B |Café, α & β | notes]] [[2025_09_30]]");
    const links = node.querySelectorAll("a");
    expect(links[0].dataset.page).toBe("A/B");
    expect(links[0].textContent).toBe("Café, α & β | notes");
    expect(links[1].textContent).toBe("2025-09-30");
  });

  it("escapes both targets and aliases as literal text", () => {
    const node = rendered('[[Page" onmouseover="bad|<img src=x onerror=bad> & "label"]]');
    const link = node.querySelector("a")!;
    expect(link.dataset.page).toBe('Page" onmouseover="bad');
    expect(link.textContent).toBe('<img src=x onerror=bad> & "label"');
    expect(link.hasAttribute("onmouseover")).toBe(false);
    expect(node.querySelector("img")).toBeNull();
  });

  it("renders aliases safely in assistant answers too", () => {
    const html = renderAssistantMarkdown("[[Insulin resistance|#insulin_resistance]]");
    expect(html).toContain('data-page="Insulin resistance"');
    expect(html).toContain(">#insulin_resistance</a>");
    expect(html).not.toContain("data-tag");
  });

  it("treats alias display as literal text, not math or nested markup", () => {
    const node = rendered("[[Math|$x$ & **symbols** #tag]]");
    expect(node.querySelector("a")?.textContent).toBe("$x$ & **symbols** #tag");
    expect(node.querySelector(".katex, strong, [data-tag]")).toBeNull();
  });

  it("does not extract an alias tag from an invalid empty target", () => {
    const node = rendered("[[|#display]]");
    expect(node.textContent).toBe("[[|#display]]");
    expect(node.querySelector("[data-tag], [data-page]")).toBeNull();
  });
});

describe("Unicode tags and protected syntax", () => {
  it("renders complete Unicode tags rather than truncated pages", () => {
    const node = rendered("#健康/睡眠, #café! #cafe\u0301 #кириллица #α_β");
    expect([...node.querySelectorAll("[data-tag]")].map((a) => (a as HTMLElement).dataset.tag))
      .toEqual(["健康/睡眠", "café", "cafe\u0301", "кириллица", "α_β"]);
  });

  it.each([
    "`[[Page|#display]] #tag ((ref))`",
    "``[[Page|#display]] ` #tag``",
    "~~~\n[[Page|#display]] #tag\n~~~",
    "    [[Page|#display]] #tag\n",
    "[#citation](https://example.com/#fragment)",
    "![#image](https://example.com/#fragment)",
    "https://example.com/#fragment",
    "<https://example.com/#fragment>",
    "<code>[[Page|#display]] #tag</code>",
    "\\[[Page|display]]",
    "\\[[Page|#display]]",
    "$x + [[Page|#display]] + #tag$",
    "$$x + #tag$$",
  ])("does not invent graph links in %s", (source) => {
    const node = rendered(source);
    expect(node.querySelector("[data-page], [data-tag], [data-ref]")).toBeNull();
  });

  it("keeps external citations and block references distinct", () => {
    const node = rendered("[1](https://example.com/paper#section) ((abc-123)) [[Concept|#concept]]");
    expect(node.querySelector("a.external-link")?.getAttribute("href")).toBe("https://example.com/paper#section");
    expect(node.querySelector("[data-ref]")?.textContent).toBe("((abc-123))");
    expect(node.querySelectorAll("[data-tag]")).toHaveLength(0);
  });

  it.each(["`$x$`", "``$x$``", "~~~\n$x$\n~~~", "    $x$\n"])("preserves math-looking code: %s", (source) => {
    expect(rendered(source).querySelector(".katex")).toBeNull();
  });

  it("preserves dollar signs in external citation URLs", () => {
    const node = rendered("[paper](https://example.com/$x$#section)");
    expect(node.querySelector("a")?.getAttribute("href")).toBe("https://example.com/$x$#section");
    expect(node.querySelector(".katex, [data-tag]")).toBeNull();
  });

  it("keeps multiline display math opaque across blank lines", () => {
    const node = rendered("$$\nx + #tag\n\n+ [[Page|#display]]\n$$");
    expect(node.querySelector("[data-page], [data-tag]")).toBeNull();
    expect(node.querySelector(".katex, .katex-error")).not.toBeNull();
  });
});

describe("host-owned concept formatting", () => {
  it.each([
    [" Insulin resistance ", "#insulin_resistance"],
    ["Health/α-β & café", "#health_α_β_café"],
    ["健康 睡眠", "#健康_睡眠"],
    ["Cafe\u0301", "#cafe\u0301"],
    ["Vitamin B12 (cobalamin)", "#vitamin_b12_cobalamin"],
  ])("formats %s without changing the target title", (title, display) => {
    expect(formatConceptTag(title)).toBe(display);
    expect(formatConceptLink(title)).toBe(`[[${title.trim()}|${display}]]`);
  });

  it.each(["", "!!!", "Bad|alias", "Bad]]", "Bad\nTitle", "https://example.com/paper", "((abc-123))"])("rejects unsafe generated targets: %s", (title) => {
    expect(formatConceptLink(title)).toBeNull();
  });
});
