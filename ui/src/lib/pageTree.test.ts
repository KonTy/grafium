import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  camelToSnakeDeep,
  collectionMembersFromBlocks,
  getCollectionKind,
  getPageTree,
  isCommandNotRegistered,
  listCollections,
  setPageCollection,
  snakeToCamelDeep,
  toPageTreeView,
  withMissingCommandFallback,
  type PageTreeNode,
} from "./pageTree";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockReset();
});

describe("page tree payload casing", () => {
  it("maps nested keys without changing values", () => {
    expect(snakeToCamelDeep({
      page_id: "page_id-is-a-value",
      children: [{ page_count: 2 }],
    })).toEqual({
      pageId: "page_id-is-a-value",
      children: [{ pageCount: 2 }],
    });
  });

  it("round-trips nested payloads", () => {
    const payload = {
      page_id: "p1",
      members: [{ page_title: "Chapter One", order_index: 0 }],
    };
    expect(camelToSnakeDeep(snakeToCamelDeep(payload))).toEqual(payload);
  });

  it("maps deeply nested payloads without recursion", () => {
    const root: Record<string, unknown> = {};
    let cursor = root;
    for (let depth = 0; depth < 10_000; depth += 1) {
      const child: Record<string, unknown> = {};
      cursor.child_nodes = child;
      cursor = child;
    }
    cursor.page_id = "leaf";

    let mapped = snakeToCamelDeep<Record<string, unknown>>(root);
    for (let depth = 0; depth < 10_000; depth += 1) {
      mapped = mapped.childNodes as Record<string, unknown>;
    }
    expect(mapped.pageId).toBe("leaf");
  });
});

describe("page tree commands", () => {
  it("passes command arguments in Tauri camelCase", async () => {
    mockInvoke.mockResolvedValueOnce([]);
    await getPageTree("namespace");
    expect(mockInvoke).toHaveBeenCalledWith("pages_namespace_tree");

    mockInvoke.mockResolvedValueOnce([]);
    await getPageTree("tags");
    expect(mockInvoke).toHaveBeenCalledWith("pages_tag_tree");

    mockInvoke.mockResolvedValueOnce([]);
    await listCollections();
    expect(mockInvoke).toHaveBeenCalledWith("pages_list_collections");

    mockInvoke.mockResolvedValueOnce(undefined);
    await setPageCollection("p1", "book");
    expect(mockInvoke).toHaveBeenCalledWith("page_set_collection", {
      pageId: "p1",
      kind: "book",
    });

    mockInvoke.mockResolvedValueOnce(undefined);
    await setPageCollection("p1", null);
    expect(mockInvoke).toHaveBeenCalledWith("page_set_collection", {
      pageId: "p1",
      kind: null,
    });
  });

  it("normalizes a contract tree iteratively for the shared renderer", () => {
    const nodes: PageTreeNode[] = [{
      key: "tech",
      label: "tech",
      page_id: "p1",
      children: [{
        key: "tech/linux",
        label: "linux",
        page_id: "p2",
        children: [],
        descendant_count: 1,
      }],
      descendant_count: 2,
    }];

    expect(toPageTreeView(nodes, "namespace")).toEqual([{
      id: "namespace:tech",
      label: "tech",
      page_id: "p1",
      page_title: "tech",
      count: 2,
      children: [{
        id: "namespace:tech/linux",
        label: "linux",
        page_id: "p2",
        page_title: "tech/linux",
        count: 1,
        children: [],
      }],
    }]);
  });

  it("normalizes deeply nested payloads without recursion", () => {
    const root: PageTreeNode = {
      key: "0",
      label: "0",
      page_id: null,
      children: [],
      descendant_count: 10_001,
    };
    let cursor = root;
    for (let depth = 1; depth <= 10_000; depth += 1) {
      const child: PageTreeNode = {
        key: String(depth),
        label: String(depth),
        page_id: null,
        children: [],
        descendant_count: 10_001 - depth,
      };
      cursor.children = [child];
      cursor = child;
    }

    const [viewRoot] = toPageTreeView([root], "namespace");
    let viewCursor = viewRoot;
    for (let depth = 1; depth <= 10_000; depth += 1) {
      viewCursor = viewCursor.children[0];
    }
    expect(viewCursor.id).toBe("namespace:10000");
  });
});

describe("collection projections", () => {
  it("reads only a valid collection marker", () => {
    expect(getCollectionKind({ collection: { kind: "book" } })).toBe("book");
    expect(getCollectionKind({ collection: { status: "draft" } })).toBeNull();
    expect(getCollectionKind({ collection: "book" })).toBeNull();
    expect(getCollectionKind(null)).toBeNull();
  });

  it("projects linked blocks in their supplied tree order", () => {
    expect(collectionMembersFromBlocks([
      { id: "b2", order_index: 4, content: "Read [[Part\\Two]] then [[Appendix]]" },
      { id: "b1", order_index: 0, content: "A note without a member" },
      { id: "b3", order_index: 1, content: "[[Part Three]]" },
    ])).toEqual([
      { block_id: "b2", order_index: 4, page_title: "Part/Two" },
      { block_id: "b3", order_index: 1, page_title: "Part Three" },
    ]);
  });
});

describe("missing page tree commands", () => {
  it.each([
    "Command pages_namespace_tree not found",
    "pages_namespace_tree not allowed",
    "Unknown command: pages_namespace_tree",
    "command is not registered",
  ])("recognizes an unavailable command: %s", (message) => {
    expect(isCommandNotRegistered(message)).toBe(true);
  });

  it("returns a typed fallback only for unavailable commands", async () => {
    await expect(withMissingCommandFallback(
      () => Promise.reject(new Error("Command pages_namespace_tree not found")),
      [] as PageTreeNode[],
    )).resolves.toEqual({ available: false, value: [] });

    await expect(withMissingCommandFallback(
      () => Promise.reject(new Error("database is locked")),
      [] as PageTreeNode[],
    )).rejects.toThrow("database is locked");
  });
});
