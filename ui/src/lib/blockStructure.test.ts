import { describe, expect, it } from "vitest";
import type { Block } from "./api";
import {
  applicableRestoreMoves,
  blocksInTreeOrder,
  insertAtPosition,
  planDeleteKeepingChildren,
  withPlannedStructure,
  type BlockMove,
} from "./blockStructure";

function mk(id: string, parent_id: string | null, order_index: number): Block {
  return {
    id,
    page_id: "p",
    parent_id,
    order_index,
    content: id,
    block_type: "text",
    properties: {},
    created_at: "0",
    updated_at: "0",
  };
}

function shape(blocks: Block[]): Array<[string, string | null, number]> {
  return blocks.map((block) => [block.id, block.parent_id, block.order_index]);
}

/** Replays moves the way the native batch does, to prove undo is exact. */
function apply(blocks: readonly Block[], moves: BlockMove[], deleted: string[] = []): Block[] {
  const byId = new Map(moves.map((move) => [move.id, move]));
  return blocksInTreeOrder(blocks
    .filter((block) => !deleted.includes(block.id))
    .map((block) => {
      const move = byId.get(block.id);
      return move ? { ...block, parent_id: move.newParentId, order_index: move.orderIndex } : block;
    }));
}

const outline = () => [
  mk("a", null, 0),
  mk("parent", null, 1),
  mk("child1", "parent", 0),
  mk("grandchild", "child1", 0),
  mk("child2", "parent", 1),
  mk("b", null, 2),
];

describe("blocksInTreeOrder", () => {
  it("puts an outdented block after its old parent's remaining children", () => {
    // The editor used to keep "c" where it was in the flat list, drawing it
    // above its former sibling with the wrong guide lines until a restart.
    const flat = [mk("p", null, 0), mk("c1", "p", 0), mk("c", null, 1), mk("c2", "p", 2), mk("q", null, 2)];
    expect(blocksInTreeOrder(flat).map((block) => block.id)).toEqual(["p", "c1", "c2", "c", "q"]);
  });

  it("keeps the incoming order for equal order numbers and keeps unreachable blocks", () => {
    const flat = [mk("x", null, 0), mk("y", null, 0), mk("orphan", "missing", 0), mk("z", "y", 0)];
    expect(blocksInTreeOrder(flat).map((block) => block.id)).toEqual(["x", "y", "z", "orphan"]);
  });
});

describe("planDeleteKeepingChildren", () => {
  it("deletes only the parent and moves its children into its place", () => {
    const plan = planDeleteKeepingChildren(outline(), new Set(["parent"]));
    expect(plan.deleted.map((block) => block.id)).toEqual(["parent"]);
    expect(shape(plan.blocks)).toEqual([
      ["a", null, 0],
      ["child1", null, 1],
      ["grandchild", "child1", 0],
      ["child2", null, 2],
      ["b", null, 3],
    ]);
    expect(plan.moves).toEqual([
      { id: "child1", newParentId: null, orderIndex: 1 },
      { id: "child2", newParentId: null, orderIndex: 2 },
      { id: "b", newParentId: null, orderIndex: 3 },
    ]);
  });

  it("undo moves restore the exact original outline", () => {
    const original = outline();
    const plan = planDeleteKeepingChildren(original, new Set(["parent"]));
    const recreated = [...plan.blocks, ...plan.deleted];
    expect(shape(apply(recreated, plan.restoreMoves))).toEqual(shape(blocksInTreeOrder(original)));
    expect(shape(apply(original, plan.moves, ["parent"]))).toEqual(shape(plan.blocks));
  });

  it("lifts children of nested deleted blocks to the nearest surviving ancestor", () => {
    const plan = planDeleteKeepingChildren(outline(), new Set(["parent", "child1"]));
    expect(plan.deleted.map((block) => block.id)).toEqual(["parent", "child1"]);
    expect(shape(plan.blocks)).toEqual([
      ["a", null, 0],
      ["grandchild", null, 1],
      ["child2", null, 2],
      ["b", null, 3],
    ]);
  });

  it("never moves a block whose parent is missing from the page", () => {
    // The native batch only accepts parents on the page, so moving it would
    // make the whole delete fail.
    const blocks = [...outline(), mk("orphan", "missing-parent", 0)];
    const plan = planDeleteKeepingChildren(blocks, new Set(["parent"]));
    expect(plan.moves.some((move) => move.id === "orphan")).toBe(false);
    expect(plan.blocks.find((block) => block.id === "orphan")).toBe(blocks.at(-1));
    expect(plan.blocks.map((block) => block.id)).toEqual(["a", "child1", "grandchild", "child2", "b", "orphan"]);
  });

  it("keeps stored order numbers when nothing needs to move", () => {
    const plan = planDeleteKeepingChildren([mk("a", null, 0), mk("b", null, 5), mk("c", null, 9)], new Set(["b"]));
    expect(plan.moves).toEqual([]);
    expect(shape(plan.blocks)).toEqual([["a", null, 0], ["c", null, 9]]);
  });

  it("deleting a child only renumbers the list that receives its children", () => {
    const plan = planDeleteKeepingChildren(outline(), new Set(["child1"]));
    // "child2" already has order 1 in the renumbered list, so it is not rewritten.
    expect(plan.moves).toEqual([{ id: "grandchild", newParentId: "parent", orderIndex: 0 }]);
    expect(shape(plan.blocks).map(([id, parent]) => [id, parent])).toEqual([
      ["a", null],
      ["parent", null],
      ["grandchild", "parent"],
      ["child2", "parent"],
      ["b", null],
    ]);
  });
});

describe("ties left by older versions", () => {
  const tied = () => [mk("x", null, 0), mk("y", null, 1), mk("n", null, 1)];

  it("inserts by displayed position and numbers the list like the native insert", () => {
    // Enter at the end of "y" and Enter at the start of "n" both mean position 2.
    const after = insertAtPosition(tied(), mk("new", null, 99), 2);
    expect(shape(after)).toEqual([["x", null, 0], ["y", null, 1], ["new", null, 2], ["n", null, 3]]);
  });

  it("undo numbers the touched list in its original order", () => {
    const original = [mk("a", null, 0), mk("p", null, 1), mk("b", null, 1)];
    const plan = planDeleteKeepingChildren(original, new Set(["p"]));
    // The recreated block is newer than "b", so with old numbers it would sort after it.
    const recreated = [...plan.blocks, ...plan.deleted];
    expect(apply(recreated, plan.restoreMoves).map((block) => block.id)).toEqual(["a", "p", "b"]);
  });
});

describe("withPlannedStructure", () => {
  it("applies the planned parents and positions without replacing newer text", () => {
    const before = outline();
    const plan = planDeleteKeepingChildren(before, new Set(["parent"]));
    // "b" was saved with new text while the native change was running.
    const current = before.map((block) => block.id === "b" ? { ...block, content: "newer text" } : block);
    const next = withPlannedStructure(current, plan.blocks, new Set(["parent"]));
    expect(shape(next)).toEqual(shape(plan.blocks));
    expect(next.find((block) => block.id === "b")!.content).toBe("newer text");
    expect(next.find((block) => block.id === "a")).toBe(current[0]);
  });
});

describe("applicableRestoreMoves", () => {
  it("undo of a delete leaves blocks the user moved afterwards where they are", () => {
    const original = [mk("p", null, 0), mk("a", "p", 0), mk("d", "p", 1), mk("b", "p", 2), mk("c", "p", 3)];
    const plan = planDeleteKeepingChildren(original, new Set(["d"]));
    // After the delete the user presses Tab on "c", moving it under "b".
    const afterTab = plan.blocks.map((block) => block.id === "c" ? { ...block, parent_id: "b", order_index: 0 } : block);
    const recreated = [...afterTab, ...plan.deleted];
    const moves = applicableRestoreMoves(recreated, plan.restoreMoves);
    expect(moves.map((move) => move.id)).toEqual(["a", "d", "b"]);
    expect(moves.every((move) => !("parentAfterDelete" in move))).toBe(true);
    const restored = apply(recreated, moves);
    expect(restored.map((block) => [block.id, block.parent_id])).toEqual([
      ["p", null], ["a", "p"], ["d", "p"], ["b", "p"], ["c", "b"],
    ]);
  });

  it("drops moves for blocks or parents that no longer exist", () => {
    const original = outline();
    const plan = planDeleteKeepingChildren(original, new Set(["parent"]));
    const recreated = [...plan.blocks.filter((block) => block.id !== "child2"), ...plan.deleted];
    expect(applicableRestoreMoves(recreated, plan.restoreMoves).some((move) => move.id === "child2")).toBe(false);
    expect(applicableRestoreMoves(plan.blocks, plan.restoreMoves).some((move) => move.newParentId === "parent")).toBe(false);
  });
});

