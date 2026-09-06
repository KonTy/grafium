import { describe, it, expect } from "vitest";
import { planIndentSelection } from "./blockIndent";
import type { Block } from "./api";

function mk(
  id: string,
  parent_id: string | null,
  order_index: number
): Block {
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

/** Flatten result into a readable [id, parentId] list in tree order. */
function shape(blocks: Block[]): Array<[string, string | null]> {
  return blocks.map((b) => [b.id, b.parent_id]);
}

describe("planIndentSelection", () => {
  it("single-block indent parents it under the previous sibling (matches legacy)", () => {
    const blocks = [mk("a", null, 0), mk("b", null, 1)];
    const plan = planIndentSelection(blocks, new Set(["b"]), "in");
    expect(shape(plan.blocks)).toEqual([
      ["a", null],
      ["b", "a"],
    ]);
    expect(plan.moves).toEqual([{ id: "b", newParentId: "a", newOrderIndex: 0 }]);
  });

  it("first child at top of document is a silent no-op on indent", () => {
    const blocks = [mk("a", null, 0), mk("b", null, 1)];
    const plan = planIndentSelection(blocks, new Set(["a"]), "in");
    expect(plan.moves).toEqual([]);
    expect(shape(plan.blocks)).toEqual([
      ["a", null],
      ["b", null],
    ]);
  });

  it("contiguous multi-block indent parents them all under the predecessor", () => {
    const blocks = [
      mk("a", null, 0),
      mk("b", null, 1),
      mk("c", null, 2),
      mk("d", null, 3),
    ];
    const plan = planIndentSelection(blocks, new Set(["b", "c", "d"]), "in");
    expect(shape(plan.blocks)).toEqual([
      ["a", null],
      ["b", "a"],
      ["c", "a"],
      ["d", "a"],
    ]);
    // relative order preserved under the new parent
    const kids = plan.blocks.filter((x) => x.parent_id === "a");
    expect(kids.map((k) => [k.id, k.order_index])).toEqual([
      ["b", 0],
      ["c", 1],
      ["d", 2],
    ]);
  });

  it("non-contiguous selection indents each contiguous run independently", () => {
    // a, b(sel), c, d(sel), e(sel)
    const blocks = [
      mk("a", null, 0),
      mk("b", null, 1),
      mk("c", null, 2),
      mk("d", null, 3),
      mk("e", null, 4),
    ];
    const plan = planIndentSelection(blocks, new Set(["b", "d", "e"]), "in");
    // b goes under a; d,e go under c
    expect(shape(plan.blocks)).toEqual([
      ["a", null],
      ["b", "a"],
      ["c", null],
      ["d", "c"],
      ["e", "c"],
    ]);
  });

  it("outdent reverses a group indent cleanly", () => {
    // a -> {b,c,d}
    const blocks = [
      mk("a", null, 0),
      mk("b", "a", 0),
      mk("c", "a", 1),
      mk("d", "a", 2),
    ];
    const plan = planIndentSelection(blocks, new Set(["b", "c", "d"]), "out");
    expect(shape(plan.blocks)).toEqual([
      ["a", null],
      ["b", null],
      ["c", null],
      ["d", null],
    ]);
    expect(plan.blocks.map((x) => [x.id, x.order_index])).toEqual([
      ["a", 0],
      ["b", 1],
      ["c", 2],
      ["d", 3],
    ]);
  });

  it("indent then outdent round-trips to the original structure", () => {
    const start = [mk("a", null, 0), mk("b", null, 1), mk("c", null, 2)];
    const indented = planIndentSelection(start, new Set(["b", "c"]), "in");
    expect(shape(indented.blocks)).toEqual([
      ["a", null],
      ["b", "a"],
      ["c", "a"],
    ]);
    const back = planIndentSelection(indented.blocks, new Set(["b", "c"]), "out");
    expect(shape(back.blocks)).toEqual([
      ["a", null],
      ["b", null],
      ["c", null],
    ]);
  });

  it("a selected child of a selected block travels with its parent (not moved twice)", () => {
    // a, b(sel) -> child(sel), and predecessor a
    const blocks = [
      mk("a", null, 0),
      mk("b", null, 1),
      mk("child", "b", 0),
    ];
    const plan = planIndentSelection(blocks, new Set(["b", "child"]), "in");
    // only b is reparented under a; child stays under b
    expect(plan.moves).toEqual([{ id: "b", newParentId: "a", newOrderIndex: 0 }]);
    expect(shape(plan.blocks)).toEqual([
      ["a", null],
      ["b", "a"],
      ["child", "b"],
    ]);
  });

  it("outdent of a top-level unit is a silent no-op", () => {
    const blocks = [mk("a", null, 0), mk("b", null, 1)];
    const plan = planIndentSelection(blocks, new Set(["a", "b"]), "out");
    expect(plan.moves).toEqual([]);
  });
});

describe("planIndentSelection — persistence scope", () => {
  // Regression: the planner used to renumber every block in the page, so any
  // page whose stored order_index values weren't already dense (the normal
  // case) emitted a move for essentially every block — a 4-block indent
  // produced 40+ backend writes and felt sluggish.
  const sparse = (): Block[] => [
    mk("a", null, 0),
    mk("b", null, 10),
    mk("c", null, 20),
    mk("d", null, 30),
    mk("e", null, 40),
  ];

  it("only writes blocks in lists the move actually touched", () => {
    const plan = planIndentSelection(sparse(), new Set(["c"]), "in");
    expect(plan.moves.map((m) => m.id).sort()).toEqual(["c"]);
  });

  it("leaves untouched siblings on their original order_index", () => {
    const plan = planIndentSelection(sparse(), new Set(["c"]), "in");
    const e = plan.blocks.find((b) => b.id === "e")!;
    expect(e.order_index).toBe(40);
  });

  it("does not renumber a sibling list that never changed", () => {
    const blocks: Block[] = [
      mk("p1", null, 0),
      mk("p1c1", "p1", 5),
      mk("p1c2", "p1", 15),
      mk("p2", null, 10),
      mk("p2c1", "p2", 7),
      mk("p2c2", "p2", 21),
    ];
    const plan = planIndentSelection(blocks, new Set(["p2c2"]), "in");
    // p1's children were never involved, so they must not be rewritten.
    expect(plan.moves.some((m) => m.id.startsWith("p1c"))).toBe(false);
  });
});
