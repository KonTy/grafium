import type { Block } from "./api";

export interface BlockRenderState {
  blockById: Map<string, Block>;
  parentById: Map<string, string | null>;
  childrenByParent: Map<string | null, Block[]>;
  depthById: Map<string, number>;
  /// Whether a block is the last child among its siblings (last item in
  /// `childrenByParent.get(parentId)`). Used to decide whether a hierarchy
  /// "thread" guide line should keep running past this block's row for each
  /// of its ancestor levels — see `getAncestorGuides`.
  isLastChildById: Map<string, boolean>;
  visibleIds: Set<string>;
  visibleBlocks: Block[];
  visibleIndexById: Map<string, number>;
}

export interface VirtualWindowOptions {
  scrollTop: number;
  viewportHeight: number;
  measuredHeights?: ReadonlyMap<string, number>;
  defaultHeight: number;
  overscanPx: number;
  anchorIndex?: number | null;
  /// Widens the computed range so it never shrinks below
  /// `[minStartIndex, minEndIndex)` even if the scroll-position-derived
  /// window would otherwise be narrower. Used to keep blocks that are part
  /// of an in-progress native text-selection drag mounted in the DOM —
  /// unmounting a block that holds the selection's anchor/focus node while
  /// the browser is auto-scrolling makes the selection visibly snap back
  /// instead of extending smoothly. Both bounds are optional and clamped to
  /// valid item indices.
  minStartIndex?: number | null;
  minEndIndex?: number | null;
}

export interface VirtualWindow<T> {
  startIndex: number;
  endIndex: number;
  topSpacer: number;
  bottomSpacer: number;
  totalHeight: number;
  items: T[];
}

export function buildBlockRenderState(
  blocks: readonly Block[],
  collapsedIds: ReadonlySet<string>
): BlockRenderState {
  const blockById = new Map<string, Block>();
  const parentById = new Map<string, string | null>();
  const childrenByParent = new Map<string | null, Block[]>();

  for (const block of blocks) {
    blockById.set(block.id, block);
    parentById.set(block.id, block.parent_id);
    const siblings = childrenByParent.get(block.parent_id) ?? [];
    siblings.push(block);
    childrenByParent.set(block.parent_id, siblings);
  }

  const depthById = new Map<string, number>();
  const visibilityById = new Map<string, boolean>();
  const depthStack = new Set<string>();
  const visibilityStack = new Set<string>();

  const getDepth = (blockId: string): number => {
    const cached = depthById.get(blockId);
    if (cached !== undefined) return cached;
    if (depthStack.has(blockId)) return 0;

    depthStack.add(blockId);
    const parentId = parentById.get(blockId) ?? null;
    const depth = parentId ? getDepth(parentId) + 1 : 0;
    depthStack.delete(blockId);
    depthById.set(blockId, depth);
    return depth;
  };

  const isVisible = (blockId: string): boolean => {
    const cached = visibilityById.get(blockId);
    if (cached !== undefined) return cached;
    if (visibilityStack.has(blockId)) return true;

    visibilityStack.add(blockId);
    const parentId = parentById.get(blockId) ?? null;
    const visible = parentId ? !collapsedIds.has(parentId) && isVisible(parentId) : true;
    visibilityStack.delete(blockId);
    visibilityById.set(blockId, visible);
    return visible;
  };

  const visibleIds = new Set<string>();
  const visibleBlocks: Block[] = [];
  const visibleIndexById = new Map<string, number>();

  for (const block of blocks) {
    getDepth(block.id);
    if (!isVisible(block.id)) continue;
    visibleIndexById.set(block.id, visibleBlocks.length);
    visibleIds.add(block.id);
    visibleBlocks.push(block);
  }

  const isLastChildById = new Map<string, boolean>();
  for (const siblings of childrenByParent.values()) {
    siblings.forEach((sibling, i) => {
      isLastChildById.set(sibling.id, i === siblings.length - 1);
    });
  }

  return {
    blockById,
    parentById,
    childrenByParent,
    depthById,
    isLastChildById,
    visibleIds,
    visibleBlocks,
    visibleIndexById,
  };
}

/// Computes, for a block, whether a vertical "thread" guide line should be
/// drawn at each of its ancestor indent levels (index 0 = root level, index
/// `depth - 1` = the block's immediate parent's level). A level's guide is
/// drawn (true) as long as the ancestor at that depth still has a later
/// sibling somewhere in the tree — i.e. more content will appear in that
/// column further down the page — matching the classic outliner
/// "bullet-threading" visual (e.g. Logseq's dev-theme bullet threading).
export function getAncestorGuides(
  blockId: string,
  parentById: ReadonlyMap<string, string | null>,
  depthById: ReadonlyMap<string, number>,
  isLastChildById: ReadonlyMap<string, boolean>
): boolean[] {
  const depth = depthById.get(blockId) ?? 0;
  const guides = new Array<boolean>(depth);
  let ancestorId: string | null = blockId;
  for (let level = depth - 1; level >= 0; level--) {
    ancestorId = parentById.get(ancestorId ?? "") ?? null;
    if (ancestorId === null) break;
    guides[level] = !(isLastChildById.get(ancestorId) ?? false);
  }
  return guides;
}

export function computeVirtualWindow<T extends { id: string }>(
  items: readonly T[],
  options: VirtualWindowOptions
): VirtualWindow<T> {
  if (items.length === 0) {
    return {
      startIndex: 0,
      endIndex: 0,
      topSpacer: 0,
      bottomSpacer: 0,
      totalHeight: 0,
      items: [],
    };
  }

  const defaultHeight = Math.max(1, options.defaultHeight);
  const viewportHeight = Math.max(defaultHeight, options.viewportHeight);
  const scrollTop = Math.max(0, options.scrollTop);
  const overscanPx = Math.max(0, options.overscanPx);
  const heights = items.map((item) => normalizeHeight(options.measuredHeights?.get(item.id), defaultHeight));
  const prefixHeights = new Array<number>(items.length + 1);
  prefixHeights[0] = 0;
  for (let i = 0; i < heights.length; i += 1) {
    prefixHeights[i + 1] = prefixHeights[i] + heights[i];
  }

  let range = getWindowRange(prefixHeights, scrollTop, viewportHeight, overscanPx);

  if (
    typeof options.anchorIndex === "number" &&
    options.anchorIndex >= 0 &&
    options.anchorIndex < items.length &&
    (options.anchorIndex < range.startIndex || options.anchorIndex >= range.endIndex)
  ) {
    const anchorTop = prefixHeights[options.anchorIndex];
    const anchorHeight = heights[options.anchorIndex];
    const centeredScrollTop = Math.max(0, anchorTop - Math.max(0, (viewportHeight - anchorHeight) / 2));
    range = getWindowRange(prefixHeights, centeredScrollTop, viewportHeight, overscanPx);
  }

  if (typeof options.minStartIndex === "number" || typeof options.minEndIndex === "number") {
    const clampedMinStart = Math.max(0, Math.min(items.length - 1, options.minStartIndex ?? range.startIndex));
    const clampedMinEnd = Math.max(1, Math.min(items.length, options.minEndIndex ?? range.endIndex));
    range = {
      startIndex: Math.min(range.startIndex, clampedMinStart),
      endIndex: Math.max(range.endIndex, clampedMinEnd),
    };
  }

  const totalHeight = prefixHeights[prefixHeights.length - 1];

  return {
    startIndex: range.startIndex,
    endIndex: range.endIndex,
    topSpacer: prefixHeights[range.startIndex],
    bottomSpacer: totalHeight - prefixHeights[range.endIndex],
    totalHeight,
    items: items.slice(range.startIndex, range.endIndex),
  };
}

function normalizeHeight(height: number | undefined, defaultHeight: number): number {
  if (typeof height === "number" && Number.isFinite(height) && height > 0) {
    return height;
  }
  return defaultHeight;
}

function getWindowRange(
  prefixHeights: readonly number[],
  scrollTop: number,
  viewportHeight: number,
  overscanPx: number
): { startIndex: number; endIndex: number } {
  const itemCount = prefixHeights.length - 1;
  if (itemCount <= 0) {
    return { startIndex: 0, endIndex: 0 };
  }

  const totalHeight = prefixHeights[itemCount];
  const windowStart = Math.max(0, scrollTop - overscanPx);
  const windowEnd = Math.min(totalHeight, scrollTop + viewportHeight + overscanPx);

  const startIndex = Math.min(itemCount - 1, Math.max(0, upperBound(prefixHeights, windowStart) - 1));
  const endIndex = Math.min(itemCount, Math.max(startIndex + 1, lowerBound(prefixHeights, windowEnd)));

  return { startIndex, endIndex };
}

function lowerBound(values: readonly number[], target: number): number {
  let low = 0;
  let high = values.length;
  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (values[mid] < target) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }
  return low;
}

function upperBound(values: readonly number[], target: number): number {
  let low = 0;
  let high = values.length;
  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (values[mid] <= target) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }
  return low;
}
