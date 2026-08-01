import { describe, expect, it } from "vitest";
import type { Block } from "./api";
import { buildBlockRenderState, computeVirtualWindow } from "./pageContentVirtualization";

function makeBlock(
  id: string,
  parentId: string | null,
  orderIndex: number,
  content = id
): Block {
  return {
    id,
    page_id: "page-1",
    parent_id: parentId,
    order_index: orderIndex,
    content,
    block_type: "markdown",
    properties: {},
    created_at: "0",
    updated_at: "0",
  };
}

function legacyHasChildren(blocks: readonly Block[], blockId: string): boolean {
  return blocks.some((block) => block.parent_id === blockId);
}

function legacyIsVisible(blocks: readonly Block[], collapsedIds: ReadonlySet<string>, block: Block): boolean {
  let parentId = block.parent_id;
  while (parentId) {
    if (collapsedIds.has(parentId)) return false;
    const parent = blocks.find((candidate) => candidate.id === parentId);
    parentId = parent?.parent_id ?? null;
  }
  return true;
}

function legacyDepth(blocks: readonly Block[], block: Block): number {
  let depth = 0;
  let parentId = block.parent_id;
  while (parentId) {
    depth += 1;
    const parent = blocks.find((candidate) => candidate.id === parentId);
    parentId = parent?.parent_id ?? null;
  }
  return depth;
}

describe("page content block render state", () => {
  it("matches the legacy children, depth, and visibility behavior", () => {
    const blocks = [
      makeBlock("root-a", null, 0),
      makeBlock("root-a-1", "root-a", 0),
      makeBlock("root-a-1-a", "root-a-1", 0),
      makeBlock("root-a-2", "root-a", 1),
      makeBlock("root-b", null, 1),
      makeBlock("root-b-1", "root-b", 0),
      makeBlock("root-b-1-a", "root-b-1", 0),
      makeBlock("root-c", null, 2),
    ];
    const collapsedIds = new Set(["root-a-1", "root-b"]);

    const state = buildBlockRenderState(blocks, collapsedIds);

    for (const block of blocks) {
      expect((state.childrenByParent.get(block.id)?.length ?? 0) > 0).toBe(
        legacyHasChildren(blocks, block.id)
      );
      expect(state.depthById.get(block.id)).toBe(legacyDepth(blocks, block));
      expect(state.visibleIds.has(block.id)).toBe(legacyIsVisible(blocks, collapsedIds, block));
    }

    expect(state.visibleBlocks.map((block) => block.id)).toEqual(
      blocks.filter((block) => legacyIsVisible(blocks, collapsedIds, block)).map((block) => block.id)
    );
  });
});

describe("page content virtual window", () => {
  it("keeps a bounded moving window no matter how far the user scrolls", () => {
    const blocks = Array.from({ length: 5000 }, (_, index) => makeBlock(`block-${index}`, null, index));
    const measuredHeights = new Map(blocks.map((block) => [block.id, 60]));
    const options = {
      viewportHeight: 600,
      measuredHeights,
      defaultHeight: 60,
      overscanPx: 240,
    };

    const nearTop = computeVirtualWindow(blocks, { ...options, scrollTop: 0 });
    const middle = computeVirtualWindow(blocks, { ...options, scrollTop: 120_000 });
    const nearBottom = computeVirtualWindow(blocks, { ...options, scrollTop: 240_000 });
    const maxWindowSize = Math.ceil((options.viewportHeight + options.overscanPx * 2) / options.defaultHeight);

    expect(nearTop.items.length).toBeLessThanOrEqual(maxWindowSize);
    expect(middle.items.length).toBeLessThanOrEqual(maxWindowSize);
    expect(nearBottom.items.length).toBeLessThanOrEqual(maxWindowSize);
    expect(middle.startIndex).toBeGreaterThan(nearTop.startIndex);
    expect(nearBottom.startIndex).toBeGreaterThan(middle.startIndex);
    expect(nearBottom.endIndex).toBeLessThanOrEqual(blocks.length);
  });

  it("can anchor a far-away block into the rendered window for navigation restores", () => {
    const blocks = Array.from({ length: 5000 }, (_, index) => makeBlock(`block-${index}`, null, index));
    const targetIndex = 4200;
    const windowed = computeVirtualWindow(blocks, {
      scrollTop: 0,
      viewportHeight: 600,
      measuredHeights: new Map(blocks.map((block) => [block.id, 60])),
      defaultHeight: 60,
      overscanPx: 240,
      anchorIndex: targetIndex,
    });

    expect(windowed.startIndex).toBeLessThanOrEqual(targetIndex);
    expect(windowed.endIndex).toBeGreaterThan(targetIndex);
    expect(windowed.items.some((block) => block.id === `block-${targetIndex}`)).toBe(true);
  });
});
