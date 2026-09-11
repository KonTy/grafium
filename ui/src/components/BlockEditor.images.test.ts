import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(join(process.cwd(), "src/components/BlockEditor.svelte"), "utf8");

describe("BlockEditor rendered images", () => {
  it("does not height-cap markdown images because that distorts aspect ratios", () => {
    const rule = source.match(/\.rendered-content :global\(\.fc-img\) \{([\s\S]*?)\n\s*\}/);

    expect(rule?.[1]).toContain("height: auto;");
    expect(rule?.[1]).not.toContain("max-height");
  });
});

describe("BlockEditor empty spacer blocks", () => {
  it("uses the visual-empty helper for marker visibility and placeholder rendering", () => {
    expect(source).toContain("let isVisuallyEmpty = $derived(isVisuallyEmptyBlock(block.content));");
    expect(source).toContain("!isVisuallyEmpty &&");
    expect(source).toContain("{#if isVisuallyEmpty}");
  });

  it("treats bare list markers as empty spacer blocks", () => {
    expect(source).toContain("return normalized === \"\" || /^[-*+]$/.test(normalized);");
  });
});

describe("BlockEditor rendered links and tables", () => {
  it("keeps page links at the surrounding text size", () => {
    const rule = source.match(/\.rendered-content :global\(\.page-link\) \{([\s\S]*?)\n\s*\}/);

    expect(rule?.[1]).toContain("font-size: inherit;");
    expect(rule?.[1]).toContain("font-weight: inherit;");
    expect(source).not.toContain(".rendered-content > :global(.page-link:only-child)");
  });

  it("suppresses the outliner bullet for table blocks", () => {
    expect(source).toContain('let isTableBlock = $derived(renderedHtml.includes("<table"));');
    expect(source).toContain("|| isTableBlock");
    expect(source).toContain("class:table-block={isTableBlock && !isEditing}");
  });
});

describe("BlockEditor Make link context menu", () => {
  it("adds a right-click action for selected editor text", () => {
    expect(source).toContain("wrapPageLinkText");
    expect(source).toContain("contextmenu: (event, view) =>");
    expect(source).toContain("function rememberEditorPageLinkRange(view: EditorView)");
    expect(source).toContain("function cachedEditorPageLinkRange(view: EditorView)");
    expect(source).toContain("function selectedRenderedPageLinkRange()");
    expect(source).toContain("Make link");
  });

  it("reports direct block text mutations for app-level undo", () => {
    expect(source).toContain("onContentChange?: (pageId: string, change: BlockContentChange) => void;");
    expect(source).toContain("function recordPersistedContentChange(");
    expect(source).toContain("const before = await getBlock(blockId);");
  });
});
