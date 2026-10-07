import { describe, expect, it } from "vitest";
import type { Block } from "./api";
import {
  chapterIdForBlock,
  chapterRange,
  flattenBlocks,
  headingText,
  listChapterHeadings,
  normalizeDepth,
  resolvePrintBlocks,
  selectionRange,
} from "./printScope";

function mk(id: string, parent_id: string | null, order_index: number, content = id): Block {
  return {
    id,
    page_id: "p",
    parent_id,
    order_index,
    content,
    block_type: "text",
    properties: {},
    created_at: "0",
    updated_at: "0",
  };
}

const ids = (entries: ReadonlyArray<{ block: Block }>): string[] =>
  entries.map((entry) => entry.block.id);

/** An imported book: one long page whose chapters are sibling `#` headings. */
function flatBook(): Block[] {
  return [
    mk("h1", null, 0, "# Chapter One"),
    mk("a", null, 1, "Opening line."),
    mk("b", null, 2, "Second line."),
    mk("h2", null, 3, "## A section"),
    mk("c", null, 4, "Section text."),
    mk("h3", null, 5, "# Chapter Two"),
    mk("d", null, 6, "Later text."),
  ];
}

/** A hand-written outline: content indented beneath each heading. */
function nestedOutline(): Block[] {
  return [
    mk("h1", null, 0, "# Chapter One"),
    mk("a", "h1", 0, "Nested line."),
    mk("a2", "a", 0, "Deeper line."),
    mk("h2", null, 1, "# Chapter Two"),
    mk("b", "h2", 0, "Other nested line."),
  ];
}

describe("flattenBlocks", () => {
  it("returns reading order with nesting depth", () => {
    expect(flattenBlocks(nestedOutline()).map((e) => [e.block.id, e.depth])).toEqual([
      ["h1", 0],
      ["a", 1],
      ["a2", 2],
      ["h2", 0],
      ["b", 1],
    ]);
  });

  it("orders siblings by order_index regardless of input order", () => {
    const shuffled = [mk("c", null, 2), mk("a", null, 0), mk("b", null, 1)];
    expect(ids(flattenBlocks(shuffled))).toEqual(["a", "b", "c"]);
  });

  it("includes collapsed and off-screen blocks, because it never reads the DOM", () => {
    // A folded parent hides its children on screen; printing must still show them.
    const blocks = [mk("parent", null, 0), mk("child", "parent", 0)];
    expect(ids(flattenBlocks(blocks))).toEqual(["parent", "child"]);
  });

  it("keeps blocks whose parent is missing instead of dropping writing", () => {
    const orphaned = [mk("kept", null, 0), mk("orphan", "gone", 0)];
    expect(ids(flattenBlocks(orphaned))).toContain("orphan");
  });

  it("does not hang on a parent cycle", () => {
    const cycle = [mk("a", "b", 0), mk("b", "a", 0)];
    expect(ids(flattenBlocks(cycle))).toHaveLength(2);
  });
});

describe("headingText", () => {
  it("strips heading marks and inline Markdown", () => {
    expect(headingText("## The **bold** `code` [[page]] title")).toBe("The bold code page title");
  });
});

describe("listChapterHeadings", () => {
  it("lists only headings, with their level", () => {
    expect(listChapterHeadings(flattenBlocks(flatBook()))).toEqual([
      { id: "h1", level: 1, text: "Chapter One" },
      { id: "h2", level: 2, text: "A section" },
      { id: "h3", level: 1, text: "Chapter Two" },
    ]);
  });

  it("is empty for a page with no headings, so no chapter scope is offered", () => {
    expect(listChapterHeadings(flattenBlocks([mk("a", null, 0, "Just text")]))).toEqual([]);
  });
});

describe("chapterRange", () => {
  it("prints a flat book chapter up to the next same-rank heading", () => {
    const chapter = chapterRange(flattenBlocks(flatBook()), "h1");
    expect(ids(chapter)).toEqual(["h1", "a", "b", "h2", "c"]);
  });

  it("keeps deeper sub-headings inside the chapter", () => {
    expect(ids(chapterRange(flattenBlocks(flatBook()), "h2"))).toEqual(["h2", "c"]);
  });

  it("runs to the end of the page for the last chapter", () => {
    expect(ids(chapterRange(flattenBlocks(flatBook()), "h3"))).toEqual(["h3", "d"]);
  });

  it("prints a nested outline chapter from its indented children", () => {
    expect(ids(chapterRange(flattenBlocks(nestedOutline()), "h1"))).toEqual(["h1", "a", "a2"]);
  });

  it("keeps nested content even when it repeats the parent's heading rank", () => {
    // Pathological, but the writing is visibly inside the chapter on screen,
    // so printing the chapter must not leave it out.
    const blocks = [
      mk("h1", null, 0, "# Outer"),
      mk("inner", "h1", 0, "# Inner"),
      mk("text", "inner", 0, "Inner text."),
      mk("h2", null, 1, "# Next"),
    ];
    expect(ids(chapterRange(flattenBlocks(blocks), "h1"))).toEqual(["h1", "inner", "text"]);
  });

  it("returns nothing for a block that is not a heading", () => {
    expect(chapterRange(flattenBlocks(flatBook()), "a")).toEqual([]);
  });

  it("returns nothing for an unknown id", () => {
    expect(chapterRange(flattenBlocks(flatBook()), "missing")).toEqual([]);
  });
});

describe("chapterIdForBlock", () => {
  it("finds the heading a block sits under", () => {
    expect(chapterIdForBlock(flattenBlocks(flatBook()), "c")).toBe("h2");
  });

  it("returns the block itself when it is a heading", () => {
    expect(chapterIdForBlock(flattenBlocks(flatBook()), "h3")).toBe("h3");
  });

  it("returns null for content before the first heading", () => {
    const blocks = [mk("intro", null, 0, "Before any heading"), mk("h", null, 1, "# Later")];
    expect(chapterIdForBlock(flattenBlocks(blocks), "intro")).toBeNull();
  });
});

describe("selectionRange", () => {
  it("includes everything nested under a selected block", () => {
    expect(ids(selectionRange(flattenBlocks(nestedOutline()), ["a"]))).toEqual(["a", "a2"]);
  });

  it("keeps reading order across separate selections", () => {
    expect(ids(selectionRange(flattenBlocks(flatBook()), ["d", "a"]))).toEqual(["a", "d"]);
  });

  it("is empty when nothing is selected", () => {
    expect(selectionRange(flattenBlocks(flatBook()), [])).toEqual([]);
  });
});

describe("normalizeDepth", () => {
  it("brings the outermost blocks flush left and keeps relative nesting", () => {
    const nested = selectionRange(flattenBlocks(nestedOutline()), ["a"]);
    expect(normalizeDepth(nested).map((e) => [e.block.id, e.depth])).toEqual([
      ["a", 0],
      ["a2", 1],
    ]);
  });
});

describe("resolvePrintBlocks", () => {
  it("prints the whole page by default", () => {
    expect(ids(resolvePrintBlocks(flatBook(), { kind: "page" }))).toHaveLength(7);
  });

  it("prints a chapter flush left", () => {
    const chapter = resolvePrintBlocks(nestedOutline(), { kind: "chapter", chapterId: "h1" });
    expect(chapter.map((e) => [e.block.id, e.depth])).toEqual([
      ["h1", 0],
      ["a", 1],
      ["a2", 2],
    ]);
  });

  it("prints nothing for a chapter scope with no chapter chosen", () => {
    expect(resolvePrintBlocks(flatBook(), { kind: "chapter", chapterId: null })).toEqual([]);
  });

  it("prints a selection flush left", () => {
    const selected = resolvePrintBlocks(nestedOutline(), { kind: "selection", selectedIds: ["a"] });
    expect(selected.map((e) => [e.block.id, e.depth])).toEqual([
      ["a", 0],
      ["a2", 1],
    ]);
  });
});
