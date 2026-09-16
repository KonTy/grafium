import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  createBlocks: vi.fn(),
  deleteBlocks: vi.fn(),
  listBlocks: vi.fn(),
  updateBlock: vi.fn(),
  acceptLinkCandidate: vi.fn(),
  undoLinkCandidateAccept: vi.fn(),
}));
vi.mock("./knowledge", () => ({
  aiUndoSummaryInsert: vi.fn(),
  aiReapplySummaryInsert: vi.fn(),
}));
vi.mock("./toast.svelte", () => ({
  showToast: vi.fn(),
  describeError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));

import { createBlocks, deleteBlocks, listBlocks, updateBlock, type Block } from "./api";
import { deleteBlockSelection, type SelectionBlockGroup } from "./blockSelectionActions";
import { canRedo, canUndo, getUndoStackSize, performRedo, performUndo, removeUndoCallback, setUndoCallback } from "./undoStack";
import { showToast } from "./toast.svelte";

const db = new Map<string, Block>();
const disk = new Map<string, Block[]>();
let failure: { operation: "create" | "delete" | "write"; pageId: string; after: number } | undefined;
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value));
const pageBlocks = (pageId: string) => [...db.values()].filter((block) => block.page_id === pageId);

function block(id: string, pageId: string, overrides: Partial<Block> = {}): Block {
  return {
    id, page_id: pageId, parent_id: null, order_index: 0, content: id,
    block_type: "Text", properties: {}, created_at: "0", updated_at: "0", ...overrides,
  };
}

function failAt(operation: "create" | "delete" | "write", pageId: string, after: number) {
  if (failure?.operation === operation && failure.pageId === pageId && failure.after === after) {
    failure = undefined;
    throw new Error(`${operation} failed after ${after} rows`);
  }
}

function persist(pageId: string) {
  failAt("write", pageId, 0);
  disk.set(pageId, clone(pageBlocks(pageId)));
}

function seed(...blocks: Block[]) {
  for (const item of blocks) db.set(item.id, clone(item));
  for (const pageId of new Set(blocks.map((item) => item.page_id))) persist(pageId);
}

const group = (pageId: string, ...blocks: Block[]): SelectionBlockGroup => ({ pageId, blocks });
const comparable = (blocks: Block[]) => blocks
  .map(({ created_at, updated_at, ...item }) => item)
  .sort((a, b) => a.id.localeCompare(b.id));

beforeEach(() => {
  vi.resetAllMocks();
  (globalThis as any).__undoStack = [];
  (globalThis as any).__redoStack = [];
  db.clear();
  disk.clear();
  failure = undefined;
  vi.mocked(listBlocks).mockImplementation(async (pageId) => clone(pageBlocks(pageId)));
  // Match Graph's nontransactional batches: each row is committed before the
  // final file write, IDs must be unique, and parents must already exist.
  vi.mocked(createBlocks).mockImplementation(async (pageId, specs) => {
    failAt("create", pageId, 0);
    const created: Block[] = [];
    for (const [index, spec] of specs.entries()) {
      const id = spec.id ?? `generated-${index}`;
      if (db.has(id)) throw new Error(`Duplicate block ID ${id}`);
      const parentId = spec.parentIndex === undefined ? spec.parentId ?? null : created[spec.parentIndex].id;
      if (parentId && !db.has(parentId)) throw new Error(`Missing parent ${parentId}`);
      const inputType = spec.blockType ?? "text";
      const blockType = ({ text: "Text", handwriting: "Handwriting", audio: "Audio", mixed: "Mixed", flashcard: "Flashcard", query: "Query" } as Record<string, string>)[inputType] ?? "Text";
      const item = block(id, pageId, {
        parent_id: parentId, order_index: spec.orderIndex, content: spec.content,
        block_type: blockType, properties: clone(spec.properties ?? {}),
      });
      db.set(id, item);
      created.push(item);
      failAt("create", pageId, index + 1);
    }
    persist(pageId);
    return clone(created);
  });
  vi.mocked(deleteBlocks).mockImplementation(async (pageId, ids) => {
    failAt("delete", pageId, 0);
    const selected = new Set(ids);
    for (const id of ids) {
      if (!db.has(id)) throw new Error(`Missing block ID ${id}`);
      if (db.get(id)!.page_id !== pageId) throw new Error("Wrong page");
    }
    let changed = true;
    while (changed) {
      changed = false;
      for (const item of pageBlocks(pageId)) {
        if (item.parent_id && selected.has(item.parent_id) && !selected.has(item.id)) {
          selected.add(item.id);
          changed = true;
        }
      }
    }
    const removed = [...selected].map((id) => db.get(id)!);
    for (const [index, item] of removed.entries()) {
      db.delete(item.id);
      failAt("delete", pageId, index + 1);
    }
    persist(pageId);
    return clone(removed);
  });
  vi.mocked(updateBlock).mockImplementation(async (id, content, properties) => {
    const item = db.get(id);
    if (!item) throw new Error(`Missing block ID ${id}`);
    item.content = content;
    if (properties) item.properties = clone(properties);
    persist(item.page_id);
  });
});

afterEach(() => {
  removeUndoCallback("day-1");
  removeUndoCallback("day-2");
  removeUndoCallback("day-3");
});

describe("deleteBlockSelection", () => {
  it("roundtrips native PascalCase block types and removes a native Text placeholder", async () => {
    const original = block("original", "day-1", { block_type: "Query" });
    const blank = block("blank", "day-2", { content: "", block_type: "Text" });
    seed(original, blank);
    await deleteBlockSelection([group("day-1", original), group("day-2", blank)]);
    expect(pageBlocks("day-1")[0].block_type).toBe("Text");
    expect(pageBlocks("day-2")[0].block_type).toBe("Text");
    await expect(performUndo()).resolves.toBe(true);
    expect(comparable([...db.values()])).toEqual(comparable([original, blank]));
    await expect(performRedo()).resolves.toBe(true);
    await expect(performUndo()).resolves.toBe(true);
    expect(comparable([...db.values()])).toEqual(comparable([original, blank]));
  });

  it("deletes two days as one action and repeatedly restores exact IDs, nesting, order, and properties", async () => {
    const parent = block("parent", "day-1", { order_index: 4, properties: { collapsed: true, tags: ["one"] } });
    const child = block("child", "day-1", { parent_id: parent.id, order_index: 2, block_type: "Query", content: "latest saved edit" });
    const sibling = block("sibling", "day-1", { order_index: 8 });
    const nextDay = block("next", "day-2", { properties: { color: "red" } });
    seed(parent, child, sibling, nextDay);
    const before = comparable([...db.values()]);
    const cb1 = vi.fn();
    const cb2 = vi.fn();
    setUndoCallback("day-1", cb1);
    setUndoCallback("day-2", cb2);

    await deleteBlockSelection([group("day-1", child, parent), group("day-2", nextDay)]);
    expect(getUndoStackSize()).toBe(1);
    expect(db.has(parent.id)).toBe(false);
    expect(pageBlocks("day-2")).toHaveLength(1);
    expect(pageBlocks("day-2")[0].content).toBe("");
    for (let cycle = 0; cycle < 2; cycle++) {
      await expect(performUndo()).resolves.toBe(true);
      expect(comparable([...db.values()])).toEqual(before);
      expect(comparable(disk.get("day-1")!)).toEqual(comparable([parent, child, sibling]));
      expect(comparable(disk.get("day-2")!)).toEqual(comparable([nextDay]));
      expect(canUndo()).toBe(false);
      expect(canRedo()).toBe(true);
      await expect(performRedo()).resolves.toBe(true);
      expect(getUndoStackSize()).toBe(1);
      expect(pageBlocks("day-2")).toHaveLength(1);
      expect(db.has(child.id)).toBe(false);
    }
    expect(cb1).toHaveBeenCalledTimes(5);
    expect(cb2).toHaveBeenCalledTimes(5);
    expect(cb2.mock.calls[0][0].pageId).toBe("day-1");
  });

  it("coalesces repeated groups/IDs and deep-copies snapshots", async () => {
    const first = block("first", "day-1", { properties: { nested: { value: "before" } } });
    const second = block("second", "day-1", { order_index: 1 });
    seed(first, second);
    await deleteBlockSelection([group("day-1", first), group("day-1", clone(first), second)]);
    first.content = "mutated caller";
    (first.properties.nested as any).value = "after";
    expect(getUndoStackSize()).toBe(1);
    expect(deleteBlocks).toHaveBeenCalledTimes(1);
    await expect(performUndo()).resolves.toBe(true);
    expect(db.get("first")!.content).toBe("first");
    expect(db.get("first")!.properties).toEqual({ nested: { value: "before" } });
  });

  it("compares JSON properties independent of native object key order", async () => {
    const selected = block("selected", "day-1", { properties: { nested: { z: 1, a: 2 }, x: true } });
    seed(selected);
    const reordered = { ...selected, properties: { x: true, nested: { a: 2, z: 1 } } };
    await deleteBlockSelection([group("day-1", selected, reordered)]);
    await expect(performUndo()).resolves.toBe(true);
    expect(db.get(selected.id)!.properties).toEqual(selected.properties);
  });

  it("treats an empty selection as a no-op without changing history", async () => {
    await deleteBlockSelection([]);
    await deleteBlockSelection([group("day-1")]);
    expect(getUndoStackSize()).toBe(0);
    expect(listBlocks).not.toHaveBeenCalled();
    expect(deleteBlocks).not.toHaveBeenCalled();
  });

  it("rejects mismatched pages and conflicting duplicates before touching the graph", async () => {
    const item = block("a", "day-1");
    await expect(deleteBlockSelection([group("day-2", item)])).rejects.toThrow("does not belong");
    await expect(deleteBlockSelection([group("day-1", item, { ...item, content: "different" })])).rejects.toThrow("Conflicting snapshots");
    await expect(deleteBlockSelection([group("", item)])).rejects.toThrow("no ID");
    expect(listBlocks).not.toHaveBeenCalled();
    expect(getUndoStackSize()).toBe(0);
  });

  it("rejects incomplete subtrees, stale snapshots, and parent cycles before mutation", async () => {
    const parent = block("parent", "day-1");
    const child = block("child", "day-1", { parent_id: parent.id });
    seed(parent, child);
    await expect(deleteBlockSelection([group("day-1", parent)])).rejects.toThrow("outline has changed");
    await expect(deleteBlockSelection([group("day-1", { ...child, content: "stale" })])).rejects.toThrow("blocks have changed");
    await expect(deleteBlockSelection([group("day-1", { ...parent, parent_id: parent.id })])).rejects.toThrow("Invalid parent");
    expect(deleteBlocks).not.toHaveBeenCalled();
    expect(getUndoStackSize()).toBe(0);
  });

  it("preflights all pages before deleting the first page", async () => {
    const first = block("first", "day-1");
    seed(first);
    await expect(deleteBlockSelection([group("day-1", first), group("day-2", block("gone", "day-2"))])).rejects.toThrow("blocks have changed");
    expect(deleteBlocks).not.toHaveBeenCalled();
    expect(getUndoStackSize()).toBe(0);
  });

  it.each([0, 1, 2])("secures snapshots before a partial delete failure after %i rows and restores only missing IDs", async (after) => {
    const first = block("first", "day-1");
    const second = block("second", "day-2");
    const third = block("third", "day-2", { order_index: 1 });
    const untouched = block("untouched", "day-3");
    seed(first, second, third, untouched);
    const before = comparable([...db.values()]);
    const cb1 = vi.fn(), cb2 = vi.fn(), cb3 = vi.fn();
    setUndoCallback("day-1", cb1);
    setUndoCallback("day-2", cb2);
    setUndoCallback("day-3", cb3);
    failure = { operation: "delete", pageId: "day-2", after };
    await expect(deleteBlockSelection([group("day-1", first), group("day-2", second, third), group("day-3", untouched)])).rejects.toThrow("Undo can restore");
    expect(getUndoStackSize()).toBe(1);
    expect(db.has("first")).toBe(false);
    expect(db.has("untouched")).toBe(true);
    expect(cb1).toHaveBeenCalledTimes(1);
    expect(cb2).toHaveBeenCalledTimes(1);
    expect(cb3).toHaveBeenCalledTimes(1);
    await expect(performUndo()).resolves.toBe(true);
    expect(comparable([...db.values()])).toEqual(before);
  });

  it("retains partial restore progress and flushes Markdown on a no-missing-ID retry", async () => {
    const first = block("first", "day-1");
    const second = block("second", "day-2");
    seed(first, second);
    await deleteBlockSelection([group("day-1", first), group("day-2", second)]);
    failure = { operation: "create", pageId: "day-2", after: 1 };
    await expect(performUndo()).resolves.toBe(false);
    expect(db.has("first")).toBe(true);
    expect(db.has("second")).toBe(true);
    expect(disk.get("day-2")!.some((item) => item.id === "second")).toBe(false);
    expect(canUndo()).toBe(true);
    expect(canRedo()).toBe(false);
    const calls = vi.mocked(createBlocks).mock.calls.length;
    await expect(performUndo()).resolves.toBe(true);
    expect(createBlocks).toHaveBeenCalledTimes(calls);
    expect(comparable(disk.get("day-2")!)).toEqual(comparable([second]));
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining("Undo again to retry"), "error");
  });

  it("retries a parent-first partial restoration without duplicating its restored parent", async () => {
    const parent = block("parent", "day-1");
    const child = block("child", "day-1", { parent_id: "parent" });
    seed(parent, child);
    await deleteBlockSelection([group("day-1", child, parent)]);
    failure = { operation: "create", pageId: "day-1", after: 1 };
    await expect(performUndo()).resolves.toBe(false);
    expect(db.has("parent")).toBe(true);
    expect(db.has("child")).toBe(false);
    await expect(performUndo()).resolves.toBe(true);
    expect(comparable([...db.values()])).toEqual(comparable([parent, child]));
  });

  it("retains a partially failed redo so a second redo deletes only remaining IDs", async () => {
    const first = block("first", "day-1");
    const second = block("second", "day-2");
    seed(first, second);
    await deleteBlockSelection([group("day-1", first), group("day-2", second)]);
    await performUndo();
    failure = { operation: "delete", pageId: "day-2", after: 1 };
    await expect(performRedo()).resolves.toBe(false);
    expect(canRedo()).toBe(true);
    expect(canUndo()).toBe(false);
    const calls = vi.mocked(deleteBlocks).mock.calls.length;
    await expect(performRedo()).resolves.toBe(true);
    expect(deleteBlocks).toHaveBeenCalledTimes(calls);
    expect(getUndoStackSize()).toBe(1);
    await expect(performUndo()).resolves.toBe(true);
    expect(comparable([...db.values()])).toEqual(comparable([first, second]));
  });

  it("does not remove a placeholder the user edited", async () => {
    const original = block("original", "day-1");
    seed(original);
    await deleteBlockSelection([group("day-1", original)]);
    const placeholder = pageBlocks("day-1")[0];
    placeholder.content = "new work";
    await expect(performUndo()).resolves.toBe(true);
    expect(db.get(placeholder.id)!.content).toBe("new work");
    await expect(performRedo()).resolves.toBe(true);
    expect(pageBlocks("day-1")).toHaveLength(1);
    expect(pageBlocks("day-1")[0].content).toBe("new work");
  });

  it("does not remove a blank placeholder with new children", async () => {
    const original = block("original", "day-1");
    seed(original);
    await deleteBlockSelection([group("day-1", original)]);
    const placeholder = pageBlocks("day-1")[0];
    seed(block("new-child", "day-1", { parent_id: placeholder.id }));
    await expect(performUndo()).resolves.toBe(true);
    expect(db.has(placeholder.id)).toBe(true);
    expect(db.get("new-child")!.parent_id).toBe(placeholder.id);
  });

  it.each([0, 1])("recovers failed placeholder creation after %i rows without deleting selected content", async (after) => {
    const selected = block("selected", "day-1");
    seed(selected);
    failure = { operation: "create", pageId: "day-1", after };
    await expect(deleteBlockSelection([group("day-1", selected)])).rejects.toThrow("Undo can restore");
    expect(db.has(selected.id)).toBe(true);
    expect(deleteBlocks).not.toHaveBeenCalled();
    expect(getUndoStackSize()).toBe(1);
    await expect(performUndo()).resolves.toBe(true);
    expect(comparable([...db.values()])).toEqual(comparable([selected]));
  });

  it("does not treat a backend deletion that leaves selected rows as success", async () => {
    const selected = block("selected", "day-1");
    seed(selected);
    vi.mocked(deleteBlocks).mockResolvedValueOnce([]);
    await expect(deleteBlockSelection([group("day-1", selected)])).rejects.toThrow("could not be deleted");
    expect(getUndoStackSize()).toBe(1);
    await expect(performUndo()).resolves.toBe(true);
    expect(comparable([...db.values()])).toEqual(comparable([selected]));
  });

  it("protects new descendants from a stale redo", async () => {
    const original = block("original", "day-1");
    seed(original);
    await deleteBlockSelection([group("day-1", original)]);
    await performUndo();
    seed(block("new-child", "day-1", { parent_id: original.id }));
    await expect(performRedo()).resolves.toBe(false);
    expect(db.has("original")).toBe(true);
    expect(db.has("new-child")).toBe(true);
    expect(canRedo()).toBe(true);
  });

  it("prevents overlapping delete, undo, and redo while a selection operation is running", async () => {
    const original = block("original", "day-1");
    seed(original);
    let finish!: (blocks: Block[]) => void;
    vi.mocked(listBlocks).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const deletion = deleteBlockSelection([group("day-1", original)]);
    await expect(deleteBlockSelection([group("day-1", original)])).rejects.toThrow("still running");
    await expect(performUndo()).resolves.toBe(false);
    await expect(performRedo()).resolves.toBe(false);
    finish([original]);
    await deletion;
    expect(getUndoStackSize()).toBe(1);
    await expect(performUndo()).resolves.toBe(true);
  });

  it("keeps disk-write failures recoverable even after all rows were restored", async () => {
    const selected = block("selected", "day-1");
    const survivor = block("survivor", "day-1", { order_index: 1 });
    seed(selected, survivor);
    await deleteBlockSelection([group("day-1", selected)]);
    failure = { operation: "write", pageId: "day-1", after: 0 };
    await expect(performUndo()).resolves.toBe(false);
    expect(db.has("selected")).toBe(true);
    expect(disk.get("day-1")!.some((item) => item.id === "selected")).toBe(false);
    await expect(performUndo()).resolves.toBe(true);
    expect(comparable(disk.get("day-1")!)).toEqual(comparable([selected, survivor]));
  });
});
