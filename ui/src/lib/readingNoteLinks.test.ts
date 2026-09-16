import { describe, expect, it, vi } from "vitest";
import { renderBlock } from "./markdown";
import { extractMarkdownReferences } from "./markdown";
import { renderedReadingText } from "./renderedReadingText";
import { openReadingNoteFromEvent, protectReadingNotePointer } from "./readingNoteLinks";

function rendered(source: string) {
  const node = document.createElement("div");
  node.innerHTML = renderBlock(source);
  return node;
}

describe("managed source footnote markers", () => {
  it("renders numbered accessible references without page/tag targets", () => {
    const node = rendered("A sentence.[^grafium-note-12]");
    const ref = node.querySelector("sup a")!;
    expect(ref.textContent).toBe("12");
    expect(ref.getAttribute("aria-label")).toBe("Open reading note 12");
    expect(ref.getAttribute("data-reading-note-label")).toBe("grafium-note-12");
    expect(extractMarkdownReferences("A sentence.[^grafium-note-12]")).toEqual({ pages: [], tags: [] });
    expect(renderedReadingText(node).text).toBe("A sentence.");
    expect(renderedReadingText(rendered("A sentence. [^grafium-note-12] Next sentence.")).text).toBe("A sentence. Next sentence.");
  });

  it.each([
    "`[^grafium-note-1]`", "``[^grafium-note-1]``", "```\n[^grafium-note-1]\n```",
    "~~~text\n[^grafium-note-1]\n~~~", "    [^grafium-note-1]", "\\[^grafium-note-1]",
    "$x+[^grafium-note-1]$", "$$x+[^grafium-note-1]$$",
    "[link](https://example.test/[^grafium-note-1])", "<https://example.test/[^grafium-note-1]>",
    "https://example.test/[^grafium-note-1]", "[^author-footnote]", "[^grafium-note-0]",
    "[^grafium-note-1]: An ordinary definition without metadata",
  ])("leaves protected or unmanaged syntax uninteractive: %s", (source) => {
    expect(rendered(source).querySelector("[data-reading-note-label]")).toBeNull();
  });

  it("delegates nested-marker clicks to Notes before the editor handles them", () => {
    const node = rendered("Text[^grafium-note-2]");
    document.body.append(node);
    const requests = vi.fn();
    window.addEventListener("open-reading-note", requests);
    const editorClick = vi.fn();
    const editorPointer = vi.fn();
    node.addEventListener("pointerdown", (event) => {
      protectReadingNotePointer(event);
      if (!event.defaultPrevented) editorPointer();
    });
    node.addEventListener("click", (event) => {
      if (!openReadingNoteFromEvent(event, "source", "block")) editorClick();
    });
    const marker = node.querySelector("a")!;
    marker.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, cancelable: true }));
    marker.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(editorPointer).not.toHaveBeenCalled();
    expect(editorClick).not.toHaveBeenCalled();
    expect(requests).toHaveBeenCalledOnce();
    expect((requests.mock.calls[0][0] as CustomEvent).detail).toEqual({ pageId: "source", footnoteLabel: "grafium-note-2", blockId: "block" });
    window.removeEventListener("open-reading-note", requests);
    node.remove();
  });
});
