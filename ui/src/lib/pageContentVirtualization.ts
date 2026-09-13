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

export function nextProgressiveRenderLimit(
  itemCount: number,
  currentLimit: number,
  batchCount: number,
  forceIndex?: number | null
): number {
  const safeItemCount = Math.max(0, Math.floor(itemCount));
  if (safeItemCount === 0) return 0;

  const safeBatch = Math.max(1, Math.floor(batchCount));
  const safeCurrent = Math.max(1, Math.floor(currentLimit));
  const target = typeof forceIndex === "number" && Number.isFinite(forceIndex)
    ? Math.floor(forceIndex) + 1
    : safeCurrent + safeBatch;

  return Math.min(safeItemCount, Math.max(safeCurrent, target));
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

  for (const siblings of childrenByParent.values()) {
    // Equal order numbers are valid. Keep their incoming display order (the
    // backend already breaks ties by creation time), not UUID order.
    siblings.sort((a, b) => a.order_index - b.order_index);
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

/// Computes, for a block, whether a muted vertical nesting guide should be
/// drawn at each ancestor indent level (index 0 = root, index `depth - 1` =
/// the immediate parent). A column is drawn only while that ancestor still
/// has a later sibling — otherwise the line would hang past the subtree.
const EMPTY_GUIDES: boolean[] = [];
const GUIDE_INTERN = new Map<string, boolean[]>();

function internGuides(guides: boolean[]): boolean[] {
  const key = guides.map((flag) => (flag ? "1" : "0")).join("");
  const cached = GUIDE_INTERN.get(key);
  if (cached) return cached;
  GUIDE_INTERN.set(key, guides);
  return guides;
}

export function getAncestorGuides(
  blockId: string,
  parentById: ReadonlyMap<string, string | null>,
  depthById: ReadonlyMap<string, number>,
  isLastChildById?: ReadonlyMap<string, boolean>
): boolean[] {
  const depth = depthById.get(blockId) ?? 0;
  if (depth <= 0) return EMPTY_GUIDES;
  const guides = new Array<boolean>(depth).fill(false);
  let ancestorId: string | null = blockId;
  for (let level = depth - 1; level >= 0; level -= 1) {
    ancestorId = parentById.get(ancestorId ?? "") ?? null;
    if (ancestorId === null) break;
    guides[level] = !(isLastChildById?.get(ancestorId) ?? false);
  }
  return internGuides(guides);
}

export interface BulletThreadRole {
  /** Colored L from the parent column into this bullet (focused path only). */
  elbow: boolean;
  /** Parent column carried through an earlier sibling's entire visible subtree. */
  continuationDepth: number | null;
  /** Colored stem from an ancestor into its children. Never on the focused block. */
  stem: boolean;
}

export const NO_THREAD: BulletThreadRole = { elbow: false, continuationDepth: null, stem: false };

export function focusedPathIds(
  focusedId: string | null,
  parentById: ReadonlyMap<string, string | null>,
): Set<string> {
  const path = new Set<string>();
  let id: string | null = focusedId;
  while (id) {
    if (path.has(id)) break;
    path.add(id);
    id = parentById.get(id) ?? null;
  }
  return path;
}

// The active thread is a staircase, not a highlight of every ancestor gutter.
// Each parent-to-child edge spans earlier siblings AND their descendants, then
// turns at the child bullet. Compute it once for the full visible tree so rows
// entering the virtual window already know which column to continue.
export function buildBulletThreadRoles(
  focusedId: string | null,
  parentById: ReadonlyMap<string, string | null>,
  childrenByParent: ReadonlyMap<string | null, Block[]>,
  collapsedIds: ReadonlySet<string>,
  blockIds: readonly string[],
): Map<string, BulletThreadRole> {
  const roles = new Map<string, BulletThreadRole>();
  if (!focusedId) return roles;
  const visibleIds = new Set(blockIds);
  const path = [...focusedPathIds(focusedId, parentById)].reverse();
  if (parentById.get(path[0]) !== null) return roles;

  // A collapsed/hidden target has no drawable endpoint.
  if (path.some((id, index) => !visibleIds.has(id) || (index < path.length - 1 && collapsedIds.has(id)))) {
    return roles;
  }

  const visited = new Set(path);
  for (let depth = 0; depth < path.length; depth += 1) {
    const id = path[depth];
    const childId = path[depth + 1];
    roles.set(id, { elbow: depth > 0, continuationDepth: null, stem: childId !== undefined });
    if (childId === undefined) break;

    const siblings = childrenByParent.get(id) ?? [];
    for (const sibling of siblings) {
      if (sibling.id === childId) break;
      const pending = [sibling.id];
      while (pending.length > 0) {
        const precedingId = pending.pop()!;
        if (visited.has(precedingId) || !visibleIds.has(precedingId)) continue;
        visited.add(precedingId);
        roles.set(precedingId, { elbow: false, continuationDepth: depth, stem: false });
        if (!collapsedIds.has(precedingId)) {
          for (const child of childrenByParent.get(precedingId) ?? []) pending.push(child.id);
        }
      }
    }
  }
  return roles;
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
