import type { Block } from "./api";

/**
 * A single reparent/reorder operation to apply against the backend via
 * `moveBlock(id, newParentId, newOrderIndex)`.
 */
export interface IndentMove {
  id: string;
  newParentId: string | null;
  newOrderIndex: number;
}

export interface IndentPlan {
  /** Moves to persist (only blocks whose parent or order actually changed). */
  moves: IndentMove[];
  /** Updated flat block array in tree order, ready to assign to state. */
  blocks: Block[];
}

/**
 * Compute how a multi-block selection should indent ("in") or outdent ("out")
 * as a group, preserving relative structure.
 *
 * Rules (see feature spec):
 *  - Only "selection roots" move — a selected block whose parent is also
 *    selected travels with its parent, so it is ignored here.
 *  - Contiguous same-parent selected roots form an "indent unit".
 *  - "in": each unit is reparented under the sibling immediately preceding the
 *    unit's first block. A unit whose first block is the first child (no
 *    predecessor) stays put (silent no-op).
 *  - "out": each unit's blocks become siblings of their current parent, placed
 *    directly after it. Units already at the top level stay put.
 *
 * The returned `blocks` is a fresh array in tree order with `parent_id` and
 * `order_index` densely renumbered; `moves` contains only the blocks that
 * actually changed.
 */
export function planIndentSelection(
  blocks: readonly Block[],
  selectedIds: ReadonlySet<string>,
  direction: "in" | "out"
): IndentPlan {
  const blockById = new Map<string, Block>();
  for (const b of blocks) blockById.set(b.id, b);

  // Preserve the caller's array position so equal order_index values keep a
  // stable, deterministic order (mirrors the backend tree-order load).
  const arrayIndex = new Map<string, number>();
  blocks.forEach((b, i) => arrayIndex.set(b.id, i));

  const orderedChildren = (): Map<string | null, string[]> => {
    const map = new Map<string | null, string[]>();
    for (const b of blocks) {
      const arr = map.get(b.parent_id) ?? [];
      arr.push(b.id);
      map.set(b.parent_id, arr);
    }
    for (const arr of map.values()) {
      arr.sort((a, c) => {
        const oa = blockById.get(a)!.order_index;
        const oc = blockById.get(c)!.order_index;
        if (oa !== oc) return oa - oc;
        return arrayIndex.get(a)! - arrayIndex.get(c)!;
      });
    }
    return map;
  };

  const original = orderedChildren();
  const working = new Map<string | null, string[]>();
  for (const [k, v] of original) working.set(k, [...v]);

  const parentOf = (id: string): string | null => blockById.get(id)!.parent_id;
  const isSelected = (id: string) => selectedIds.has(id);
  const isSelectedRoot = (id: string) => {
    if (!isSelected(id)) return false;
    const p = parentOf(id);
    return !(p !== null && isSelected(p));
  };

  // Build contiguous same-parent runs of selected roots.
  interface Unit {
    parentId: string | null;
    ids: string[];
  }
  const units: Unit[] = [];
  for (const [parentId, childIds] of original) {
    let run: string[] = [];
    for (const id of childIds) {
      if (isSelectedRoot(id)) {
        run.push(id);
      } else if (run.length) {
        units.push({ parentId, ids: run });
        run = [];
      }
    }
    if (run.length) units.push({ parentId, ids: run });
  }

  const newParent = new Map<string, string | null>();

  const removeFrom = (parentId: string | null, ids: string[]) => {
    const list = working.get(parentId) ?? [];
    const idSet = new Set(ids);
    working.set(
      parentId,
      list.filter((x) => !idSet.has(x))
    );
  };

  if (direction === "in") {
    for (const unit of units) {
      const siblings = original.get(unit.parentId)!;
      const firstIdx = siblings.indexOf(unit.ids[0]);
      const predecessor = firstIdx > 0 ? siblings[firstIdx - 1] : null;
      if (!predecessor) continue; // no valid predecessor — leave in place
      removeFrom(unit.parentId, unit.ids);
      const predList = working.get(predecessor) ?? [];
      predList.push(...unit.ids);
      working.set(predecessor, predList);
      for (const id of unit.ids) newParent.set(id, predecessor);
    }
  } else {
    // Outdent: merge all selected roots of a given parent and drop them in
    // right after that parent, preserving their relative order.
    const byParent = new Map<string | null, string[]>();
    for (const unit of units) {
      if (unit.parentId === null) continue; // already top level
      const acc = byParent.get(unit.parentId) ?? [];
      acc.push(...unit.ids);
      byParent.set(unit.parentId, acc);
    }
    for (const [parentId, ids] of byParent) {
      const grandparent = parentOf(parentId!);
      removeFrom(parentId, ids);
      const gList = working.get(grandparent) ?? [];
      const pIdx = gList.indexOf(parentId!);
      const insertAt = pIdx >= 0 ? pIdx + 1 : gList.length;
      gList.splice(insertAt, 0, ...ids);
      working.set(grandparent, gList);
      for (const id of ids) newParent.set(id, grandparent);
    }
  }

  // Flatten into tree order and renumber densely.
  const outBlocks: Block[] = [];
  const moves: IndentMove[] = [];
  const visit = (parentId: string | null) => {
    const kids = working.get(parentId) ?? [];
    kids.forEach((id, index) => {
      const orig = blockById.get(id)!;
      const np = newParent.has(id) ? newParent.get(id)! : orig.parent_id;
      const updated: Block = { ...orig, parent_id: np, order_index: index };
      outBlocks.push(updated);
      if (orig.parent_id !== np || orig.order_index !== index) {
        moves.push({ id, newParentId: np, newOrderIndex: index });
      }
      visit(id);
    });
  };
  visit(null);

  return { moves, blocks: outBlocks };
}
