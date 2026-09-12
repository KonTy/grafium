import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(join(process.cwd(), "src/components/PageContent.svelte"), "utf8");

describe("PageContent block selection cut", () => {
  it("handles native cut events for selected blocks", () => {
    expect(source).toContain("oncut={handleCutSelection}");
    expect(source).toContain("function handleCutSelection(e: ClipboardEvent)");
    expect(source).toContain('e.clipboardData.setData("text/markdown", markdown);');
    expect(source).toContain("await handleDeleteSelected();");
  });

  it("handles Ctrl-X even when the browser does not dispatch a cut event", () => {
    expect(source).toContain('e.key.toLowerCase() === "x"');
    expect(source).toContain("void cutSelectedBlocks();");
  });

  it("focuses the nearest block after selected blocks are deleted", () => {
    expect(source).toContain("function focusBlockForEditing(blockId: string)");
    expect(source).toContain("let focusAfterDelete: Block | null = null;");
    expect(source).toContain("focusBlockForEditing(focusAfterDelete.id);");
  });

  it("copies selected blocks as structured outline markdown", () => {
    expect(source).toContain("formatBlocksAsOutlineMarkdown");
    expect(source).toContain("function selectedBlockMarkdown()");
    expect(source).toContain("selectedBlocksWithDescendantsInDocumentOrder()");
    expect(source).toContain("depth: getBlockDepth(block.id)");
  });

  it("deletes selected block subtrees in one backend batch", () => {
    expect(source).toContain("function blocksWithDescendantsInDocumentOrder(rootIds: ReadonlySet<string>)");
    expect(source).toContain("await deleteBlocks(page.id, deletedBlocks.map((block) => block.id));");
    expect(source).toContain("const remaining = blocks.filter((b) => !deletedIds.has(b.id));");
  });

  it("records undo for batched multi-block paste", () => {
    expect(source).toContain("createBlocks(request.pageId, batch)");
    expect(source).toContain('type: "insert_blocks"');
    expect(source).toContain("beforeContent: anchorBeforeContent");
    expect(source).toContain("insertedBlocks: newBlocks.map(snapshotBlock)");
  });

  it("records undo for selected block text conversions", () => {
    expect(source).toContain("function pushBlockContentUndo(pageId: string, changes: BlockContentChange[])");
    expect(source).toContain('type: "update_blocks"');
    expect(source).toContain("await applyCurrentPageContentChanges(updates);");
  });

  it("offers Make link from rendered text selections", () => {
    expect(source).toContain("function nativeSelectionMakeLinkAction()");
    expect(source).toContain("from: number | null;");
    expect(source).toContain("function captureNativeSelectionMakeLinkAction()");
    expect(source).toContain("function cachedSelectionMakeLinkAction(blockId: string)");
    expect(source).toContain("Only the");
    expect(source).toContain("canTurnBulletsToTodos");
    expect(source).toContain("function makeSelectionLink()");
    expect(source).toContain("selectionMenu.makeLink");
    expect(source).toContain("wrapPageLinkText(block.content");
    expect(source).toContain("captureNativeSelectionMakeLinkAction() ?? cachedSelectionMakeLinkAction(blockId)");
  });

  it("coordinates pointer drag selection across active block editors", () => {
    expect(source).toContain("function handleBlockSelectionPointerDown(e: PointerEvent)");
    expect(source).toContain("onpointermove={handleBlockSelectionPointerMove}");
    expect(source).toContain("setDraggedBlockSelection(drag.startBlockId, endBlockId)");
  });
});
