// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { get } from "svelte/store";
import { renderBlock } from "./markdown";
import { renderedReadingText } from "./renderedReadingText";
import {
  captureRenderedReadingSelection, continuousReadingSelection, publishSourceReadingSelection,
  readingSelection, readingSelectionCapture, sourceReadingSelection,
} from "./readingSelection";

function rendered(pageId: string, blockId: string, html: string, unified = false): string {
  return unified
    ? `<div class="unified-page-editor" data-page-id="${pageId}"><div data-source-block-id="${blockId}"><div class="rendered-content">${html}</div></div></div>`
    : `<div data-page-id="${pageId}" data-block-id="${blockId}"><div class="rendered-content">${html}</div></div>`;
}

function select(start: Node, from: number, end: Node, to: number): Selection {
  const selection = window.getSelection()!;
  const range = document.createRange();
  range.setStart(start, from);
  range.setEnd(end, to);
  selection.removeAllRanges();
  selection.addRange(range);
  return selection;
}

beforeEach(() => {
  window.getSelection()?.removeAllRanges();
  document.body.innerHTML = "";
  readingSelection.set({ selection: null, error: null, pageIds: [] });
});

describe("reading selections", () => {
  it("preserves exact source text, block offsets, and anchoring context", () => {
    const source = "Before **exact** text after";
    const selected = sourceReadingSelection("page", "block", source, 7, 16)!;
    expect(selected.text).toBe("**exact**");
    expect(selected.parts[0]).toMatchObject({ from: 7, to: 16, prefix: "Before ", suffix: " text after" });
    expect(sourceReadingSelection("page", "block", source, 8, 8)).toBeNull();
  });

  it("keeps Unicode context intact when a window ends inside a surrogate pair", () => {
    const emoji = "\u{1F600}";
    const source = emoji.repeat(40) + "xneedley" + emoji.repeat(40);
    const selected = sourceReadingSelection("page", "block", source, 81, 87)!;
    expect(selected.parts[0]).toEqual({
      blockId: "block", text: "needle", from: 81, to: 87,
      prefix: emoji.repeat(31) + "x", suffix: "y" + emoji.repeat(31),
    });
  });

  it("maps a continuous cross-block range without including metadata or widening the source", () => {
    const source = "- Alpha text\n  id:: a\n- Beta text\n  more text\n  id:: b\n- Not selected\n  id:: c";
    const from = source.indexOf("text");
    const to = source.indexOf("more") + 4;
    const captured = continuousReadingSelection("page", source, from, to).selection!;
    expect(captured.blockIds).toEqual(["a", "b"]);
    expect(captured.text).toBe("text\nBeta text\nmore");
    expect(captured.documentRange).toEqual({ from, to, text: source.slice(from, to) });
    expect(captured.parts[1]).toMatchObject({ from: 0, to: 14, text: "Beta text\nmore" });
  });

  it("rejects unsaved continuous blocks rather than associating them with another block", () => {
    const captured = continuousReadingSelection("page", "- Unsaved content", 2, 17);
    expect(captured.selection).toBeNull();
    expect(captured.error).toMatch(/no saved block ID.*classic editor/);
  });

  for (const unified of [false, true]) {
    it(`captures ${unified ? "continuous" : "classic"} rendered selections including nested formatting`, () => {
      document.body.innerHTML = `<main class="main-content"><div class="page-content">${rendered("page", "block", "Before <strong>exact</strong> text", unified)}</div></main>`;
      const text = document.querySelector("strong")!.firstChild!;
      const captured = captureRenderedReadingSelection(select(text, 1, text, 4))!.selection!;
      expect(captured.pageId).toBe("page");
      expect(captured.blockIds).toEqual(["block"]);
      expect(captured.text).toBe("xac");
      expect(captured.parts[0]).toMatchObject({ from: 8, to: 11, prefix: "Before e", suffix: "t text" });
    });
  }

  it("freezes a rendered quote before pane focus clears the DOM selection", () => {
    document.body.innerHTML = `<main class="main-content"><div class="page-content">${rendered("page", "a", "A quotation")}</div></main><aside><button>Panel</button></aside>`;
    const action = readingSelectionCapture(document.querySelector(".page-content")!);
    const text = document.querySelector(".rendered-content")!.firstChild!;
    select(text, 2, text, 11);
    document.querySelector("button")!.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    window.getSelection()!.removeAllRanges();
    document.dispatchEvent(new Event("selectionchange"));
    expect(get(readingSelection).selection?.text).toBe("quotation");
    action.destroy();
  });

  it("captures only selected portions of multiple blocks, not bullets or the whole page", () => {
    document.body.innerHTML = `<main class="main-content"><div class="page-content">${rendered("page", "a", "First passage")}${rendered("page", "b", "Next passage")}${rendered("page", "c", "Unselected")}</div></main>`;
    const content = document.querySelectorAll(".rendered-content");
    const result = captureRenderedReadingSelection(select(content[0].firstChild!, 6, content[1].firstChild!, 4))!;
    expect(result.selection?.text).toBe("passage\nNext");
    expect(result.selection?.blockIds).toEqual(["a", "b"]);
  });

  it("rejects selections spanning journal days", () => {
    document.body.innerHTML = `<main class="main-content"><div class="page-content">${rendered("day-a", "a", "First day")}</div><div class="page-content">${rendered("day-b", "b", "Next day")}</div></main>`;
    const content = document.querySelectorAll(".rendered-content");
    const result = captureRenderedReadingSelection(select(content[0].firstChild!, 2, content[1].firstChild!, 4))!;
    expect(result.selection).toBeNull();
    expect(result.pageIds).toEqual(["day-a", "day-b"]);
    expect(result.error).toMatch(/one page or journal day/);
  });

  it("ignores transcript/other-panel text even with spoofed source markers", () => {
    document.body.innerHTML = `<aside>${rendered("page", "a", "Assistant response")}</aside>`;
    const text = document.querySelector(".rendered-content")!.firstChild!;
    expect(captureRenderedReadingSelection(select(text, 0, text, 9))).toBeNull();
    publishSourceReadingSelection(document.querySelector("aside")!, sourceReadingSelection("page", "a", "Assistant response", 0, 9));
    expect(get(readingSelection).selection).toBeNull();
  });

  it("rejects a selection crossing from a note into a panel", () => {
    document.body.innerHTML = `<main class="main-content"><div class="page-content">${rendered("page", "a", "Note text")}</div></main><aside>Transcript</aside>`;
    const note = document.querySelector(".rendered-content")!.firstChild!;
    const result = captureRenderedReadingSelection(select(note, 0, document.querySelector("aside")!.firstChild!, 5))!;
    expect(result.selection).toBeNull();
    expect(result.error).toMatch(/across pages or panels/);
  });

  it("rejects keyboard block selections spanning journal days even when DOM text selection is collapsed", () => {
    document.body.innerHTML = `<main class="main-content"><div class="page-content">
      <div class="block-item selected" data-page-id="day-a" data-block-id="a"></div>
      <div class="block-item selected" data-page-id="day-b" data-block-id="b"></div>
      </div></main>`;
    const action = readingSelectionCapture(document.querySelector(".page-content")!);
    document.dispatchEvent(new Event("pointerdown"));
    expect(get(readingSelection).selection).toBeNull();
    expect(get(readingSelection).error).toMatch(/one page or journal day/);
    action.destroy();
  });

  describe("actual Markdown renderer text and anchor mapping", () => {
    function mount(markdown: string, unified = false): HTMLElement {
      document.body.innerHTML = `<main class="main-content"><div class="page-content">${rendered("page", "block", renderBlock(markdown), unified)}</div></main>`;
      return document.querySelector(".rendered-content")!;
    }

    function selectContents(content: HTMLElement) {
      return captureRenderedReadingSelection(select(content, 0, content, content.childNodes.length))!.selection!;
    }

    for (const unified of [false, true]) {
      for (const markdown of ["Alpha\nBeta", "Alpha  \nBeta", "Alpha\\\nBeta"]) {
        it(`preserves ${JSON.stringify(markdown)} logical breaks in the ${unified ? "continuous" : "classic"} renderer`, () => {
          const content = mount(markdown, unified);
          expect(content.querySelector("br")).not.toBeNull();
          const captured = selectContents(content);
          expect(captured.text).toBe("Alpha\nBeta");
          expect(captured.parts[0]).toMatchObject({ text: "Alpha\nBeta", from: 0, to: 10, prefix: "", suffix: "" });
          const last = content.lastChild!;
          const beta = captureRenderedReadingSelection(select(last, 0, last, 4))!.selection!;
          expect(beta.parts[0]).toMatchObject({ text: "Beta", from: 6, to: 10, prefix: "Alpha\n", suffix: "" });
        });
      }

      it(`preserves fenced code lines and omits language/toolbar labels in the ${unified ? "continuous" : "classic"} renderer`, () => {
        const content = mount("```rust\nAlpha\n\nBeta\n```", unified);
        const toolbar = document.createElement("div");
        toolbar.setAttribute("role", "toolbar");
        toolbar.textContent = "Copy code";
        content.querySelector(".code-block-wrapper")!.prepend(toolbar);
        const captured = selectContents(content);
        expect(captured.text).toBe("Alpha\n\nBeta");
        expect(captured.text).not.toMatch(/rust|Copy code/);
        expect(captured.parts[0]).toMatchObject({ from: 0, to: 11, prefix: "", suffix: "" });
        const lines = content.querySelectorAll(".code-line");
        const selected = captureRenderedReadingSelection(select(lines[0].firstChild!, 2, lines[2].firstChild!, 2))!.selection!;
        expect(selected.parts[0]).toMatchObject({ text: "pha\n\nBe", from: 2, to: 9, prefix: "Al", suffix: "ta" });
      });
    }

    it("keeps adjacent bold/link text joined while preserving source breaks and aliases", () => {
      const content = mount("Before **strong**word [label](https://example.test)\n[[Destination|Alias]] after");
      expect(renderedReadingText(content).text).toBe("Before strongword label\nAlias after");
      const bold = content.querySelector("strong")!.firstChild!;
      const wiki = content.querySelector(".page-link")!.firstChild!;
      const captured = captureRenderedReadingSelection(select(bold, 2, wiki, 3))!.selection!;
      expect(captured.text).toBe("rongword label\nAli");
      expect(captured.parts[0]).toMatchObject({ from: 9, to: 27, prefix: "Before st", suffix: "as after" });
    });

    it("preserves paragraph boundaries without pulling HTML formatting whitespace into offsets", () => {
      const content = mount("Alpha\n\nBeta");
      expect(selectContents(content).text).toBe("Alpha\nBeta");
      const beta = content.querySelectorAll("p")[1].firstChild!;
      const captured = captureRenderedReadingSelection(select(beta, 1, beta, 3))!.selection!;
      expect(captured.parts[0]).toMatchObject({ from: 7, to: 9, text: "et", prefix: "Alpha\nB", suffix: "a" });
    });

    it("uses the same logical representation across selected prose and code blocks", () => {
      document.body.innerHTML = `<main class="main-content"><div class="page-content">
        ${rendered("page", "a", renderBlock("Before **Alpha**\nBeta"))}
        ${rendered("page", "b", renderBlock("```text\nGamma\nDelta\n```"))}
        ${rendered("page", "c", renderBlock("Unselected text"))}
        </div></main>`;
      const alpha = document.querySelector("strong")!.firstChild!;
      const delta = document.querySelectorAll(".code-line")[1].firstChild!;
      const captured = captureRenderedReadingSelection(select(alpha, 2, delta, 3))!.selection!;
      expect(captured.blockIds).toEqual(["a", "b"]);
      expect(captured.text).toBe("pha\nBeta\nGamma\nDel");
      expect(captured.parts).toEqual([
        { blockId: "a", text: "pha\nBeta", from: 9, to: 17, prefix: "Before Al", suffix: "" },
        { blockId: "b", text: "Gamma\nDel", from: 0, to: 9, prefix: "", suffix: "ta" },
      ]);
    });
  });
});
