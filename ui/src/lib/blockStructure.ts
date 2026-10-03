import type { Block } from "./api";

/** One parent/position change for `restructureBlocks`. */
export interface BlockMove {
  id: string;
  newParentId: string | null;
  orderIndex: number;
}

/**
 * Blocks in document order: each block followed by its children, siblings by
 * `order_index`. Equal order numbers keep their incoming array position, which
 * matches how the page was loaded (the backend breaks ties by creation time).
 * Blocks whose parent is not on the page cannot be reached from the top level;
 * they are kept, after everything else, in their incoming order.
 */
export function blocksInTreeOrder(input: readonly Block[]): Block[] {
  const arrayIndex = new Map(input.map((block, index) => [block.id, index]));
  const childrenByParent = new Map<string | null, Block[]>();
  for (const block of input) {
    const siblings = childrenByParent.get(block.parent_id) ?? [];
    siblings.push(block);
    childrenByParent.set(block.parent_id, siblings);
  }
  for (const siblings of childrenByParent.values()) {
    siblings.sort((a, b) => a.order_index - b.order_index
      || (arrayIndex.get(a.id) ?? 0) - (arrayIndex.get(b.id) ?? 0));
  }

  const ordered: Block[] = [];
  const visited = new Set<string>();
  const visit = (parentId: string | null) => {
    for (const child of childrenByParent.get(parentId) ?? []) {
      if (visited.has(child.id)) continue;
      visited.add(child.id);
      ordered.push(child);
      visit(child.id);
    }
  };
  visit(null);
  if (ordered.length < input.length) {
    for (const block of input) {
      if (visited.has(block.id)) continue;
      visited.add(block.id);
      ordered.push(block);
      visit(block.id);
    }
  }
  return ordered;
}

/** An undo move, skipped if its block was moved elsewhere after the delete. */
export interface RestoreMove extends BlockMove {
  /** The block's parent right after the delete; absent for recreated blocks. */
  parentAfterDelete?: string | null;
}

export interface DeleteKeepingChildrenPlan {
  /** The blocks that remain, in tree order, with their new parent and order. */
  blocks: Block[];
  /** The deleted blocks as they were, in document order. */
  deleted: Block[];
  /** Moves to apply together with the deletion so no child is lost. */
  moves: BlockMove[];
  /**
   * Once the deleted blocks exist again, numbers every list the delete
   * touched in its original order, which puts each moved block back.
   */
  restoreMoves: RestoreMove[];
}

/**
 * Delete only the listed blocks. Each deleted block's children take its place
 * under its nearest surviving ancestor, in order, keeping their own subtrees.
 * Lists that receive children are renumbered so the saved order is exact;
 * lists that only lose blocks keep their stored order numbers.
 */
export function planDeleteKeepingChildren(
  blocks: readonly Block[],
  deleteIds: ReadonlySet<string>,
): DeleteKeepingChildrenPlan {
  const ordered = blocksInTreeOrder(blocks);
  const onPage = new Set(ordered.map((block) => block.id));
  const childrenByParent = new Map<string | null, Block[]>();
  for (const block of ordered) {
    const parentId = block.parent_id !== null && onPage.has(block.parent_id) ? block.parent_id : null;
    const siblings = childrenByParent.get(parentId) ?? [];
    siblings.push(block);
    childrenByParent.set(parentId, siblings);
  }

  const resultChildren = new Map<string | null, Block[]>();
  const renumbered = new Set<string | null>();
  const survivorsUnder = (parentId: string | null, listId: string | null): Block[] => {
    const survivors: Block[] = [];
    for (const child of childrenByParent.get(parentId) ?? []) {
      if (deleteIds.has(child.id)) {
        const promoted = survivorsUnder(child.id, listId);
        if (promoted.length) renumbered.add(listId);
        survivors.push(...promoted);
      } else {
        survivors.push(child);
        resultChildren.set(child.id, survivorsUnder(child.id, child.id));
      }
    }
    return survivors;
  };
  resultChildren.set(null, survivorsUnder(null, null));

  const remaining: Block[] = [];
  const moves: BlockMove[] = [];
  const place = (parentId: string | null) => {
    (resultChildren.get(parentId) ?? []).forEach((original, index) => {
      // A block whose parent is missing from the page is grouped with the
      // top level here but belongs to no list the backend can renumber.
      const orphan = original.parent_id !== null && !onPage.has(original.parent_id);
      const keepsParent = original.parent_id === parentId || (parentId === null && orphan);
      const orderIndex = renumbered.has(parentId) && !orphan ? index : original.order_index;
      const newParentId = keepsParent ? original.parent_id : parentId;
      if (newParentId === original.parent_id && orderIndex === original.order_index) {
        remaining.push(original);
      } else {
        remaining.push({ ...original, parent_id: newParentId, order_index: orderIndex });
        moves.push({ id: original.id, newParentId, orderIndex });
      }
      place(original.id);
    });
  };
  place(null);

  // Undo recreates the deleted blocks, then numbers every list this delete
  // touched in its original order. Numbering whole lists (rather than putting
  // back old numbers) also restores lists whose siblings shared a number,
  // where a recreated block would otherwise sort after its old neighbour.
  const touched = new Set<string | null>();
  for (const block of ordered) {
    if (deleteIds.has(block.id)) touched.add(block.parent_id);
  }
  const movedIds = new Set(moves.map((move) => move.id));
  for (const block of ordered) {
    if (movedIds.has(block.id)) touched.add(block.parent_id);
  }
  const parentAfterDelete = new Map(remaining.map((block) => [block.id, block.parent_id]));
  const restoreMoves: RestoreMove[] = [];
  for (const parentId of touched) {
    if (parentId !== null && !onPage.has(parentId)) continue;
    (childrenByParent.get(parentId) ?? [])
      .filter((block) => block.parent_id === parentId)
      .forEach((block, index) => restoreMoves.push(deleteIds.has(block.id)
        ? { id: block.id, newParentId: parentId, orderIndex: index }
        : { id: block.id, newParentId: parentId, orderIndex: index, parentAfterDelete: parentAfterDelete.get(block.id) ?? null }));
  }

  return {
    blocks: remaining,
    deleted: ordered.filter((block) => deleteIds.has(block.id)),
    moves,
    restoreMoves,
  };
}

/**
 * The undo moves that still apply to the page as it is now: the block and its
 * target parent exist, and the block is still where the delete left it. A
 * block the user moved somewhere else afterwards (Tab, Shift+Tab) stays there.
 */
export function applicableRestoreMoves(current: readonly Block[], moves: readonly RestoreMove[]): BlockMove[] {
  const parentById = new Map(current.map((block) => [block.id, block.parent_id]));
  return moves
    .filter((move) => parentById.has(move.id)
      && (move.newParentId === null || parentById.has(move.newParentId))
      && (move.parentAfterDelete === undefined || parentById.get(move.id) === move.parentAfterDelete))
    .map(({ id, newParentId, orderIndex }) => ({ id, newParentId, orderIndex }));
}

/**
 * The page after a block is inserted at `position` (0-based, in display order)
 * among its siblings: the same 0..n numbering the native insert saves.
 */
export function insertAtPosition(blocks: readonly Block[], inserted: Block, position: number): Block[] {
  const ordered = blocksInTreeOrder(blocks.filter((block) => block.id !== inserted.id));
  const siblings = ordered.filter((block) => block.parent_id === inserted.parent_id);
  const at = Math.min(Math.max(position, 0), siblings.length);
  const wanted = new Map(siblings.map((block, index) => [block.id, index < at ? index : index + 1]));
  return blocksInTreeOrder([
    ...ordered.map((block) => {
      const orderIndex = wanted.get(block.id);
      return orderIndex === undefined || orderIndex === block.order_index ? block : { ...block, order_index: orderIndex };
    }),
    { ...inserted, order_index: at },
  ]);
}

/**
 * Apply only the structure (parent and position) of a plan made before an
 * awaited native change to the blocks as they are now. Text saved while the
 * change was running must not be replaced by the plan's older copies.
 */
export function withPlannedStructure(
  current: readonly Block[],
  planned: readonly Block[],
  deletedIds: ReadonlySet<string> = new Set(),
): Block[] {
  const plannedById = new Map(planned.map((block) => [block.id, block]));
  return blocksInTreeOrder(current
    .filter((block) => !deletedIds.has(block.id))
    .map((block) => {
      const next = plannedById.get(block.id);
      return next && (next.parent_id !== block.parent_id || next.order_index !== block.order_index)
        ? { ...block, parent_id: next.parent_id, order_index: next.order_index }
        : block;
    }));
}
