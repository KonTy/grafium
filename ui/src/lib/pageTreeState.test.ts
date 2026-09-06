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
  graphScopedKey,
  SIDEBAR_TREE_STORAGE_KEY,
  filterTreeByQuery,
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

describe("graphScopedKey", () => {
  /// Expansion paths only mean anything inside their own graph. Sharing one
  /// key meant opening graph B pruned graph A's saved paths against B's tree
  /// and wrote the result back, quietly destroying A's state on every switch.
  it("gives different graphs different keys", () => {
    const a = graphScopedKey(SIDEBAR_TREE_STORAGE_KEY, "/home/me/graph-a");
    const b = graphScopedKey(SIDEBAR_TREE_STORAGE_KEY, "/home/me/graph-b");
    expect(a).not.toBe(b);
    expect(a.startsWith(SIDEBAR_TREE_STORAGE_KEY)).toBe(true);
  });

  it("is stable for the same graph", () => {
    const p = "/home/me/graph-a";
    expect(graphScopedKey(SIDEBAR_TREE_STORAGE_KEY, p)).toBe(
      graphScopedKey(SIDEBAR_TREE_STORAGE_KEY, p),
    );
  });

  it("falls back to the bare key when the graph is unknown", () => {
    expect(graphScopedKey(SIDEBAR_TREE_STORAGE_KEY, null)).toBe(SIDEBAR_TREE_STORAGE_KEY);
  });

  /// The key must not embed a filesystem path.
  it("does not leak the graph path", () => {
    expect(graphScopedKey(SIDEBAR_TREE_STORAGE_KEY, "/home/me/secret-graph")).not.toContain("secret");
  });
});

describe("saveExpansionState bounds", () => {
  /// A single deep title expands to one entry per level; unbounded, that
  /// serialized to tens of megabytes, blew the quota, and persisted nothing.
  it("caps what it persists so one absurd branch can't cost everything", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
    };
    const expanded = new Set<string>();
    let deep = "x";
    for (let i = 0; i < 5000; i++) {
      deep += "/x";
      expanded.add(deep);
    }
    expanded.add("shallow");

    expect(saveExpansionState(storage, "k", expanded)).toBe(true);
    const written = store.get("k")!;
    expect(written.length).toBeLessThan(500_000);
    expect(JSON.parse(written).expanded).toContain("shallow");
  });
});

describe("filterTreeByQuery", () => {
  const tree = [
    {
      id: "namespace:mybooks", label: "mybooks", page_id: null, page_title: "mybooks", count: 2,
      children: [
        {
          id: "namespace:mybooks/thingsilove", label: "thingsilove", page_id: "p1",
          page_title: "mybooks/thingsilove", count: 2,
          children: [
            { id: "namespace:mybooks/thingsilove/arc", label: "arc", page_id: "p2",
              page_title: "mybooks/thingsilove/arc", count: 1, children: [] },
          ],
        },
      ],
    },
    { id: "namespace:tech", label: "tech", page_id: "p3", page_title: "tech", count: 1, children: [] },
  ];

  it("returns everything for an empty query", () => {
    expect(filterTreeByQuery(tree, "")).toHaveLength(2);
  });

  /// A deep match is meaningless without its ancestors — a bare "arc" gives
  /// no clue it lives under mybooks/thingsilove.
  it("keeps the path to a deep match", () => {
    const out = filterTreeByQuery(tree, "arc");
    expect(out).toHaveLength(1);
    expect(out[0].label).toBe("mybooks");
    expect(out[0].children[0].children[0].label).toBe("arc");
  });

  it("keeps a matching node's whole subtree", () => {
    const out = filterTreeByQuery(tree, "thingsilove");
    expect(out[0].children[0].children).toHaveLength(1);
  });

  it("drops branches with no match", () => {
    const out = filterTreeByQuery(tree, "tech");
    expect(out).toHaveLength(1);
    expect(out[0].label).toBe("tech");
  });

  /// Matching the full path is what lets a query span segments.
  it("matches across path segments", () => {
    expect(filterTreeByQuery(tree, "mybooks arc")).toHaveLength(1);
  });

  it("returns nothing when nothing matches", () => {
    expect(filterTreeByQuery(tree, "zzzzz")).toEqual([]);
  });
});
