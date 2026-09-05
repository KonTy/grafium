import { describe, expect, it } from "vitest";
import {
  collectBranchIds,
  findAncestorIdsForPage,
  flattenVisibleTree,
  loadExpansionState,
  pruneExpansionState,
  reduceTreeNavigation,
  saveExpansionState,
  type StorageLike,
  type TreeNavigationItem,
} from "./pageTreeState";

interface Node {
  id: string;
  children: Node[];
}

const tree: Node[] = [
  {
    id: "tech",
    children: [
      { id: "tech/linux", children: [{ id: "tech/linux/filesystems", children: [] }] },
      { id: "tech/rust", children: [] },
    ],
  },
  { id: "writing", children: [] },
];

class MemoryStorage implements StorageLike {
  values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
}

describe("page tree flattening", () => {
  it("walks only expanded branches and preserves ARIA hierarchy metadata", () => {
    const rows = flattenVisibleTree(tree, new Set(["tech", "tech/linux"]), (node) => node.id);

    expect(rows.map((row) => row.id)).toEqual([
      "tech",
      "tech/linux",
      "tech/linux/filesystems",
      "tech/rust",
      "writing",
    ]);
    expect(rows[1]).toMatchObject({
      parent_id: "tech",
      level: 2,
      position: 1,
      set_size: 2,
      has_children: true,
    });
  });

  it("handles a very deep tree without recursive calls", () => {
    const root: Node = { id: "0", children: [] };
    const expanded = new Set<string>();
    let cursor = root;
    for (let depth = 1; depth <= 10_000; depth += 1) {
      expanded.add(cursor.id);
      const child: Node = { id: String(depth), children: [] };
      cursor.children = [child];
      cursor = child;
    }

    const rows = flattenVisibleTree([root], expanded, (node) => node.id);
    expect(rows).toHaveLength(10_001);
    expect(rows.at(-1)?.level).toBe(10_001);
  });

  it("collects branches once and prunes stale expansion ids", () => {
    const branches = collectBranchIds(tree, (node) => node.id);
    expect(branches).toEqual(new Set(["tech", "tech/linux"]));
    expect(pruneExpansionState(new Set(["tech", "missing", "writing"]), branches)).toEqual(
      new Set(["tech"]),
    );
  });

  it("finds the expansion path for a selected page", () => {
    expect(findAncestorIdsForPage(
      tree,
      "tech/linux/filesystems",
      (node) => node.id,
      (node) => node.id,
    )).toEqual(["tech", "tech/linux"]);
    expect(findAncestorIdsForPage(
      tree,
      "writing",
      (node) => node.id,
      (node) => node.id,
    )).toEqual([]);
    expect(findAncestorIdsForPage(
      tree,
      "missing",
      (node) => node.id,
      (node) => node.id,
    )).toBeNull();
  });
});

describe("page tree keyboard navigation", () => {
  const items: TreeNavigationItem[] = [
    { id: "tech", parent_id: null, has_children: true, can_activate: false },
    { id: "tech/linux", parent_id: "tech", has_children: false, can_activate: true },
    { id: "writing", parent_id: null, has_children: false, can_activate: true },
  ];

  it("moves through visible rows and jumps to the parent", () => {
    expect(reduceTreeNavigation("ArrowDown", items, "tech", new Set()).focus_id).toBe("tech/linux");
    expect(reduceTreeNavigation("ArrowUp", items, "tech/linux", new Set()).focus_id).toBe("tech");
    expect(reduceTreeNavigation("End", items, "tech", new Set()).focus_id).toBe("writing");
    expect(reduceTreeNavigation("ArrowLeft", items, "tech/linux", new Set()).focus_id).toBe("tech");
  });

  it("expands before entering a child and collapses before moving to a parent", () => {
    expect(reduceTreeNavigation("ArrowRight", items, "tech", new Set())).toMatchObject({
      expansion: "expand",
      focus_id: "tech",
    });
    expect(reduceTreeNavigation("ArrowRight", items, "tech", new Set(["tech"]))).toMatchObject({
      expansion: null,
      focus_id: "tech/linux",
    });
    expect(reduceTreeNavigation("ArrowLeft", items, "tech", new Set(["tech"]))).toMatchObject({
      expansion: "collapse",
      focus_id: "tech",
    });
  });

  it("activates pages but only toggles grouping nodes", () => {
    expect(reduceTreeNavigation("Enter", items, "tech/linux", new Set())).toMatchObject({
      activate: true,
      expansion: null,
    });
    expect(reduceTreeNavigation("Enter", items, "tech", new Set())).toMatchObject({
      activate: false,
      expansion: "toggle",
    });
  });
});

describe("page tree expansion persistence", () => {
  it("round-trips a deterministic versioned payload", () => {
    const storage = new MemoryStorage();
    expect(saveExpansionState(storage, "tree", new Set(["writing", "tech"]))).toBe(true);
    expect(storage.getItem("tree")).toBe(
      '{"version":1,"expanded":["tech","writing"]}',
    );
    expect(loadExpansionState(storage, "tree")).toEqual(new Set(["tech", "writing"]));
  });

  it("accepts the original array shape and ignores malformed state", () => {
    const storage = new MemoryStorage();
    storage.setItem("tree", '["tech",7,""]');
    expect(loadExpansionState(storage, "tree")).toEqual(new Set(["tech"]));
    storage.setItem("tree", "{broken");
    expect(loadExpansionState(storage, "tree")).toEqual(new Set());
  });

  it("degrades when storage is unavailable", () => {
    expect(loadExpansionState(null, "tree")).toEqual(new Set());
    expect(saveExpansionState(null, "tree", new Set(["tech"]))).toBe(false);
  });
});
