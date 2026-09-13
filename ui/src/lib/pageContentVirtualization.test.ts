import { describe, expect, it } from "vitest";
import type { Block } from "./api";
import {
  buildBlockRenderState,
  buildBulletThreadRoles,
  computeVirtualWindow,
  getAncestorGuides,
  focusedPathIds,
  NO_THREAD,
  nextProgressiveRenderLimit,
} from "./pageContentVirtualization";

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

  it("never unmounts already-rendered blocks below minStartIndex/minEndIndex, e.g. during a mouse-drag text selection", () => {
    const blocks = Array.from({ length: 5000 }, (_, index) => makeBlock(`block-${index}`, null, index));
    const measuredHeights = new Map(blocks.map((block) => [block.id, 60]));
    const options = {
      viewportHeight: 600,
      measuredHeights,
      defaultHeight: 60,
      overscanPx: 240,
    };

    // Simulate: user started a drag-selection somewhere near the top, then
    // the browser auto-scrolled the container down while the mouse stayed
    // pinned past the bottom edge. Without a sticky floor, the window
    // computed at the new scrollTop would drop the blocks the selection
    // anchor started in.
    const atDragStart = computeVirtualWindow(blocks, { ...options, scrollTop: 0 });
    const afterAutoscroll = computeVirtualWindow(blocks, {
      ...options,
      scrollTop: 3_000,
      minStartIndex: atDragStart.startIndex,
      minEndIndex: atDragStart.endIndex,
    });

    expect(afterAutoscroll.startIndex).toBeLessThanOrEqual(atDragStart.startIndex);
    expect(afterAutoscroll.endIndex).toBeGreaterThanOrEqual(atDragStart.endIndex);
    // The window must still have grown to cover the new scroll position too.
    const plainAfterAutoscroll = computeVirtualWindow(blocks, { ...options, scrollTop: 3_000 });
    expect(afterAutoscroll.endIndex).toBeGreaterThanOrEqual(plainAfterAutoscroll.endIndex);
    for (const block of atDragStart.items) {
      expect(afterAutoscroll.items.some((b) => b.id === block.id)).toBe(true);
    }
  });
});

describe("progressive render limit", () => {
  it("grows in bounded batches without exceeding the item count", () => {
    expect(nextProgressiveRenderLimit(500, 80, 80)).toBe(160);
    expect(nextProgressiveRenderLimit(500, 480, 80)).toBe(500);
  });

  it("can force a far-away block into the rendered prefix", () => {
    expect(nextProgressiveRenderLimit(500, 80, 80, 320)).toBe(321);
  });
});

describe("getAncestorGuides", () => {
  it("marks last siblings so the UI can draw L elbows instead of T junctions", () => {
    const blocks = [
      makeBlock("a", null, 0),
      makeBlock("a1", "a", 0),
      makeBlock("a2", "a", 1),
      makeBlock("b", null, 1),
    ];
    const state = buildBlockRenderState(blocks, new Set());
    expect(state.isLastChildById.get("a1")).toBe(false);
    expect(state.isLastChildById.get("a2")).toBe(true);
    expect(state.isLastChildById.get("a")).toBe(false);
    expect(state.isLastChildById.get("b")).toBe(true);
  });

  it("sorts siblings by order_index before last-child flags", () => {
    const blocks = [
      makeBlock("a2", "a", 1),
      makeBlock("a", null, 0),
      makeBlock("a1", "a", 0),
    ];
    const state = buildBlockRenderState(blocks, new Set());
    expect(state.childrenByParent.get("a")?.map((block) => block.id)).toEqual(["a1", "a2"]);
    expect(state.isLastChildById.get("a1")).toBe(false);
    expect(state.isLastChildById.get("a2")).toBe(true);
  });

  it("draws no guides for a root-level block", () => {
    const blocks = [makeBlock("a", null, 0)];
    const state = buildBlockRenderState(blocks, new Set());
    const guides = getAncestorGuides("a", state.parentById, state.depthById, state.isLastChildById);
    expect(guides).toEqual([]);
  });

  it("keeps displayed sibling order and last-child flags when order numbers are equal", () => {
    const blocks = [
      makeBlock("parent", null, 0),
      { ...makeBlock("z-earlier", "parent", 1), created_at: "100" },
      { ...makeBlock("a-later", "parent", 1), created_at: "200" },
    ];
    const state = buildBlockRenderState(blocks, new Set());
    expect(state.childrenByParent.get("parent")?.map((block) => block.id)).toEqual(["z-earlier", "a-later"]);
    expect(state.visibleBlocks.map((block) => block.id)).toEqual(blocks.map((block) => block.id));
    expect(state.isLastChildById.get("z-earlier")).toBe(false);
    expect(state.isLastChildById.get("a-later")).toBe(true);
  });

  it("draws a guide for a parent that has a later sibling", () => {
    // a
    //   b (child of a)
    // c (sibling of a, comes after)
    const blocks = [makeBlock("a", null, 0), makeBlock("b", "a", 0), makeBlock("c", null, 1)];
    const state = buildBlockRenderState(blocks, new Set());
    const guides = getAncestorGuides("b", state.parentById, state.depthById, state.isLastChildById);
    // b's only ancestor level (level 0, "a") should draw a guide since "a" has
    // a later sibling ("c") — more content still follows in that column.
    expect(guides).toEqual([true]);
  });

  it("does not draw a guide when the ancestor is the last child", () => {
    // a
    //   b (child of a, and a is an only child overall)
    const blocks = [makeBlock("a", null, 0), makeBlock("b", "a", 0)];
    const state = buildBlockRenderState(blocks, new Set());
    const guides = getAncestorGuides("b", state.parentById, state.depthById, state.isLastChildById);
    expect(guides).toEqual([false]);
  });

  it("draws a guide only while a later sibling still exists in that column", () => {
    // a
    //   b
    //     c
    // d (sibling of a, after)
    const blocks = [
      makeBlock("a", null, 0),
      makeBlock("b", "a", 0),
      makeBlock("c", "b", 0),
      makeBlock("d", null, 1),
    ];
    const state = buildBlockRenderState(blocks, new Set());
    const guides = getAncestorGuides("c", state.parentById, state.depthById, state.isLastChildById);
    expect(guides).toEqual([true, false]);
  });
});

describe("buildBulletThreadRoles", () => {
  it("does not skip or extend past equal-order siblings with IDs in reverse display order", () => {
    const blocks = [
      makeBlock("root", null, 0),
      makeBlock("parent", "root", 0),
      makeBlock("first", "parent", 0),
      makeBlock("z-earlier", "parent", 1),
      makeBlock("a-later", "parent", 1),
      makeBlock("last", "parent", 2),
    ];
    const state = buildBlockRenderState(blocks, new Set());
    const rolesFor = (id: string) => buildBulletThreadRoles(
      id, state.parentById, state.childrenByParent, new Set(), [...state.visibleIds],
    );
    const later = rolesFor("a-later");
    expect(later.get("z-earlier")).toEqual({ elbow: false, continuationDepth: 1, stem: false });
    expect(later.get("a-later")).toEqual({ elbow: true, continuationDepth: null, stem: false });
    const earlier = rolesFor("z-earlier");
    expect(earlier.get("z-earlier")).toEqual({ elbow: true, continuationDepth: null, stem: false });
    expect(earlier.has("a-later")).toBe(false);
    expect(earlier.has("last")).toBe(false);
  });

  it("colors only the focused path and prior siblings, with no tail on the focused block", () => {
    const blocks = [
      makeBlock("a", null, 0),
      makeBlock("a1", "a", 0),
      makeBlock("a2", "a", 1),
      makeBlock("a2x", "a2", 0),
      makeBlock("a3", "a", 2),
      makeBlock("b", null, 1),
    ];
    const state = buildBlockRenderState(blocks, new Set());
    const roles = buildBulletThreadRoles("a2x", state.parentById, state.childrenByParent, new Set(), blocks.map((block) => block.id));
    const role = (id: string) => roles.get(id) ?? NO_THREAD;

    expect(role("a")).toEqual({ elbow: false, continuationDepth: null, stem: true });
    expect(role("a1")).toEqual({ elbow: false, continuationDepth: 0, stem: false });
    expect(role("a2")).toEqual({ elbow: true, continuationDepth: null, stem: true });
    expect(role("a2x")).toEqual({ elbow: true, continuationDepth: null, stem: false });
    expect(role("a3")).toEqual(NO_THREAD);
    expect(role("b")).toEqual(NO_THREAD);
  });

  const nestedBlocks = [
    makeBlock("other-root", null, 0),
    makeBlock("a", null, 1),
    makeBlock("prior", "a", 0),
    makeBlock("prior-child", "prior", 0),
    makeBlock("prior-grandchild", "prior-child", 0),
    makeBlock("b", "a", 1),
    makeBlock("b-prior", "b", 0),
    makeBlock("b-prior-child", "b-prior", 0),
    makeBlock("c", "b", 1),
    makeBlock("c-prior", "c", 0),
    makeBlock("focus", "c", 1),
    makeBlock("focus-child", "focus", 0),
    makeBlock("later", "c", 2),
    makeBlock("last-root", null, 2),
  ];

  function nestedRoles(focusedId: string | null = "focus", collapsedIds = new Set<string>()) {
    const state = buildBlockRenderState(nestedBlocks, collapsedIds);
    return buildBulletThreadRoles(focusedId, state.parentById, state.childrenByParent, collapsedIds, [...state.visibleIds]);
  }

  it("carries exactly one parent column through preceding subtrees, including virtualized rows", () => {
    const roles = nestedRoles();
    for (const id of ["prior", "prior-child", "prior-grandchild"]) {
      expect(roles.get(id)).toEqual({ elbow: false, continuationDepth: 0, stem: false });
    }
    for (const id of ["b-prior", "b-prior-child"]) {
      expect(roles.get(id)).toEqual({ elbow: false, continuationDepth: 1, stem: false });
    }
    expect(roles.get("c-prior")).toEqual({ elbow: false, continuationDepth: 2, stem: false });
    expect(roles.has("other-root")).toBe(false);
    expect(roles.has("later")).toBe(false);
    expect(roles.has("focus-child")).toBe(false);
    expect(roles.has("last-root")).toBe(false);
  });

  it("turns at each path ancestor without continuing the old column below its bullet", () => {
    const roles = nestedRoles();
    for (const id of ["b", "c"]) {
      expect(roles.get(id)).toEqual({ elbow: true, continuationDepth: null, stem: true });
    }
    expect(roles.get("focus")).toEqual({ elbow: true, continuationDepth: null, stem: false });
  });

  it("stops at a parent when focus moves up and does not retain the previous path", () => {
    nestedRoles();
    const roles = nestedRoles("b");
    expect(roles.get("b")).toEqual({ elbow: true, continuationDepth: null, stem: false });
    for (const id of ["b-prior", "b-prior-child", "c", "c-prior", "focus"]) {
      expect(roles.has(id)).toBe(false);
    }
    expect(nestedRoles("a").get("a")).toEqual(NO_THREAD);
    expect(nestedRoles("a").size).toBe(1);
  });

  it("skips collapsed preceding descendants and rejects an endpoint hidden by collapse", () => {
    const roles = nestedRoles("focus", new Set(["prior"]));
    expect(roles.get("prior")?.continuationDepth).toBe(0);
    expect(roles.has("prior-child")).toBe(false);
    expect(roles.has("prior-grandchild")).toBe(false);
    expect(nestedRoles("focus", new Set(["b"])).size).toBe(0);
    expect(nestedRoles("b", new Set(["b"])).get("b")?.stem).toBe(false);
  });

  it("uses sorted tree order, not the input array or virtual window order", () => {
    const state = buildBlockRenderState([...nestedBlocks].reverse(), new Set());
    const roles = buildBulletThreadRoles("focus", state.parentById, state.childrenByParent, new Set(), [...state.visibleIds]);
    expect(roles).toEqual(nestedRoles());
  });

  it("rejects deleted and orphaned endpoints", () => {
    expect(nestedRoles("deleted").size).toBe(0);
    const state = buildBlockRenderState([makeBlock("orphan", "missing", 0)], new Set());
    expect(buildBulletThreadRoles("orphan", state.parentById, state.childrenByParent, new Set(), ["orphan"]).size).toBe(0);
  });

  it("does not hang when parent pointers form a cycle", () => {
    const parentById = new Map<string, string | null>([
      ["a", "b"],
      ["b", "a"],
    ]);
    const started = Date.now();
    const path = focusedPathIds("a", parentById);
    expect(Date.now() - started).toBeLessThan(50);
    expect([...path].sort()).toEqual(["a", "b"]);
    expect(buildBulletThreadRoles("a", parentById, new Map(), new Set(), ["a", "b"]).size).toBe(0);
  });

  it("does not color anything when nothing is focused", () => {
    expect(nestedRoles(null).size).toBe(0);
  });
});
