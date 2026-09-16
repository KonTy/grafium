import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the tauri command wrappers used by the insert_summary undo path so
// tests never touch the real invoke() bridge.
vi.mock("./knowledge", () => ({
  aiUndoSummaryInsert: vi.fn(),
  aiReapplySummaryInsert: vi.fn(),
}));

vi.mock("./api", () => ({
  acceptLinkCandidate: vi.fn(),
  createBlock: vi.fn(),
  createBlocks: vi.fn(),
  deleteBlock: vi.fn(),
  deleteBlocks: vi.fn(),
  getGraphInfo: vi.fn(),
  listBlocks: vi.fn(),
  undoLinkCandidateAccept: vi.fn(),
  updateBlock: vi.fn(),
}));

vi.mock("./toast.svelte", () => ({
  showToast: vi.fn(),
  describeError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("./writing", () => ({ applyWritingChanges: vi.fn() }));
import { applyWritingChanges } from "./writing";

import {
  aiReapplySummaryInsert,
  aiUndoSummaryInsert,
  type SummaryWrapChange,
  type AiInsertSummaryResult,
} from "./knowledge";
import {
  acceptLinkCandidate,
  createBlocks,
  deleteBlocks,
  getGraphInfo,
  listBlocks,
  undoLinkCandidateAccept,
  updateBlock,
  type Block,
} from "./api";
import {
  APP_UNDO_LIMIT,
  getUndoStackSize,
  performRedo,
  performUndo,
  pushUndo,
  setUndoCallback,
  removeUndoCallback,
} from "./undoStack";
import { showToast } from "./toast.svelte";
import { registerEditorFlush } from "./editorPersistence";

const mockUndo = vi.mocked(aiUndoSummaryInsert);
const mockReapply = vi.mocked(aiReapplySummaryInsert);
const mockCreateBlocks = vi.mocked(createBlocks);
const mockDeleteBlocks = vi.mocked(deleteBlocks);
const mockAcceptLinkCandidate = vi.mocked(acceptLinkCandidate);
const mockUndoLinkCandidateAccept = vi.mocked(undoLinkCandidateAccept);
const mockUpdateBlock = vi.mocked(updateBlock);

function block(overrides: Partial<Block> & Pick<Block, "id" | "content">): Block {
  return {
    id: overrides.id,
    page_id: overrides.page_id ?? "page-1",
    parent_id: overrides.parent_id ?? null,
    order_index: overrides.order_index ?? 0,
    content: overrides.content,
    block_type: overrides.block_type ?? "text",
    properties: overrides.properties ?? {},
    created_at: overrides.created_at ?? "0",
    updated_at: overrides.updated_at ?? "0",
  };
}

function summaryReceipt(wrapChanges: SummaryWrapChange[] = []): AiInsertSummaryResult {
  return {
    graphPath: "synthetic-graph",
    pageId: "page-1",
    insertedBlockId: "b-summary",
    insertedContent: "Summary body",
    insertedAfterBlockId: "b-anchor",
    insertedBlocks: [
      block({ id: "b-summary", content: "Summary body", order_index: 1 }),
      block({ id: "b-heading", content: "### Topic", parent_id: "b-summary" }),
      block({ id: "b-body", content: "Topic body", parent_id: "b-heading" }),
    ].map((block) => ({ ...block, block_type: "Text" as const, created_at: 0, updated_at: 0 })),
    siblingOrderBefore: [{ blockId: "b-anchor", orderIndex: 0 }],
    resolvedTargets: [],
    createdTargets: [],
    unlinkedTargets: [],
    wrapChanges,
  };
}

describe("undoStack — insert_summary flow", () => {
  beforeEach(() => {
    (globalThis as any).__undoStack = [];
    (globalThis as any).__redoStack = [];
    mockUndo.mockReset();
    mockReapply.mockReset();
    mockCreateBlocks.mockReset();
    mockDeleteBlocks.mockReset();
    mockAcceptLinkCandidate.mockReset();
    mockUndoLinkCandidateAccept.mockReset();
    mockUpdateBlock.mockReset();
    vi.mocked(listBlocks).mockReset();
    vi.mocked(getGraphInfo).mockReset().mockResolvedValue({ path: "synthetic-graph" } as any);
    vi.mocked(showToast).mockClear();
    vi.mocked(applyWritingChanges).mockReset();
  });

  it("undoes and redoes a natural rewrite through one guarded native operation", async () => {
    const changes = [
      { blockId: "one", beforeContent: "Original one", afterContent: "Natural one" },
      { blockId: "two", beforeContent: "Original two", afterContent: "Natural two" },
    ];
    pushUndo({ type: "rewrite_writing", graphPath: "/tmp/writing", pageId: "page-1", changes });
    vi.mocked(applyWritingChanges).mockResolvedValue(undefined);
    expect(await performUndo()).toBe(true);
    expect(applyWritingChanges).toHaveBeenCalledOnce();
    expect(applyWritingChanges).toHaveBeenCalledWith("/tmp/writing", "page-1", [
      { blockId: "one", beforeContent: "Natural one", afterContent: "Original one" },
      { blockId: "two", beforeContent: "Natural two", afterContent: "Original two" },
    ]);
    expect(mockUpdateBlock).not.toHaveBeenCalled();
    expect(await performRedo()).toBe(true);
    expect(applyWritingChanges).toHaveBeenLastCalledWith("/tmp/writing", "page-1", changes);
  });

  it("retains a conflicting rewrite undo and surfaces the failure without partial writes", async () => {
    pushUndo({
      type: "rewrite_writing", graphPath: "/tmp/writing", pageId: "page-1",
      changes: [{ blockId: "one", beforeContent: "Original", afterContent: "Natural" }],
    });
    vi.mocked(applyWritingChanges).mockRejectedValue(new Error("The block changed"));
    expect(await performUndo()).toBe(false);
    expect(getUndoStackSize()).toBe(1);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining("The block changed"), "error");
    expect(mockUpdateBlock).not.toHaveBeenCalled();
  });

  it("reverses the summary insert and calls the page's reload callback", async () => {
    mockUndo.mockResolvedValue({ retainedTargets: [] });
    const cb = vi.fn();
    setUndoCallback("page-1", cb);

    const wrapChanges: SummaryWrapChange[] = [
      { blockId: "b-existing", previousContent: "before", newContent: "after" },
    ];
    pushUndo({
      type: "insert_summary",
      ...summaryReceipt(wrapChanges),
    });

    try {
      const ok = await performUndo();
      expect(ok).toBe(true);
      expect(mockUndo).toHaveBeenCalledWith({ type: "insert_summary", ...summaryReceipt(wrapChanges) });
      expect(cb).toHaveBeenCalledTimes(1);
    } finally {
      removeUndoCallback("page-1");
    }
  });

  it("re-adds the action to the undo stack when the backend fails, so a follow-up Ctrl-Z isn't lost", async () => {
    mockUndo.mockRejectedValue(new Error("boom"));
    pushUndo({
      type: "insert_summary",
      ...summaryReceipt(),
    });

    const ok = await performUndo();
    expect(ok).toBe(false);
    expect((globalThis as any).__undoStack.length).toBe(1);
  });

  it("redo restores the complete tree receipt with stable identities", async () => {
    // Set up: pretend we already went summary-insert → undo, so the redo
    // stack has one entry.
    const wrapChanges: SummaryWrapChange[] = [
      { blockId: "b-existing", previousContent: "before", newContent: "after" },
    ];
    (globalThis as any).__redoStack = [
      {
        type: "insert_summary",
        ...summaryReceipt(wrapChanges),
      },
    ];

    mockReapply.mockResolvedValue(summaryReceipt(wrapChanges));

    const cb = vi.fn();
    setUndoCallback("page-1", cb);

    try {
      const ok = await performRedo();
      expect(ok).toBe(true);
      expect(mockReapply).toHaveBeenCalledWith({ type: "insert_summary", ...summaryReceipt(wrapChanges) });
      const undoStack = (globalThis as any).__undoStack;
      expect(undoStack.length).toBe(1);
      expect(undoStack[0]).toEqual({ type: "insert_summary", ...summaryReceipt(wrapChanges) });
      expect(cb).toHaveBeenCalledTimes(1);
    } finally {
      removeUndoCallback("page-1");
    }
  });

  it("flushes pending source drafts before summary undo and refuses a failed flush", async () => {
    const receipt = summaryReceipt();
    const order: string[] = [];
    let fail = true;
    const unregister = registerEditorFlush(receipt.pageId, async () => {
      order.push("flush");
      if (fail) throw new Error("Unsaved draft could not be persisted");
    });
    mockUndo.mockImplementation(async () => { order.push("undo"); return { retainedTargets: [] }; });
    pushUndo({ type: "insert_summary", ...receipt });
    try {
      expect(await performUndo()).toBe(false);
      expect(mockUndo).not.toHaveBeenCalled();
      expect(getUndoStackSize()).toBe(1);
      expect(showToast).toHaveBeenCalledWith(expect.stringContaining("Unsaved draft"), "error");
      fail = false;
      expect(await performUndo()).toBe(true);
      expect(order).toEqual(["flush", "flush", "undo"]);
    } finally {
      unregister();
    }
  });

  it("keeps stale summary redo retryable without creating flat blocks", async () => {
    const action = { type: "insert_summary" as const, ...summaryReceipt() };
    (globalThis as any).__redoStack = [action];
    mockReapply.mockRejectedValue(new Error("Summary edit is stale: insertion location changed"));
    expect(await performRedo()).toBe(false);
    expect((globalThis as any).__redoStack).toEqual([action]);
    expect(getUndoStackSize()).toBe(0);
    expect(mockCreateBlocks).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining("stale"), "error");
  });

  it("refuses summary undo on a different graph before flushing any drafts", async () => {
    const flush = vi.fn(async () => {});
    const unregister = registerEditorFlush("page-1", flush);
    vi.mocked(getGraphInfo).mockResolvedValue({ path: "different-graph" } as any);
    pushUndo({ type: "insert_summary", ...summaryReceipt() });
    try {
      expect(await performUndo()).toBe(false);
      expect(flush).not.toHaveBeenCalled();
      expect(mockUndo).not.toHaveBeenCalled();
      expect(getUndoStackSize()).toBe(1);
    } finally {
      unregister();
    }
  });

  it("reports retained concept pages without losing the successful summary undo", async () => {
    const receipt = summaryReceipt();
    receipt.createdTargets = [{
      id: "created-concept", title: "New concept", file_path: null, created_at: 42, updated_at: 42,
      is_journal: false, properties: {},
    }];
    mockUndo.mockResolvedValue({
      retainedTargets: [{ pageId: "created-concept", title: "New concept", reason: "Referenced by another note." }],
    });
    pushUndo({ type: "insert_summary", ...receipt });
    expect(await performUndo()).toBe(true);
    expect(getUndoStackSize()).toBe(0);
    expect((globalThis as any).__redoStack[0]).toEqual({ type: "insert_summary", ...receipt });
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining("Kept concept pages edited or used elsewhere: New concept"), "info");
    expect(mockDeleteBlocks).not.toHaveBeenCalled();
  });

  it("undoes and redoes an AI block replacement", async () => {
    mockUpdateBlock.mockResolvedValue(undefined);
    const cb = vi.fn();
    const replacements: Array<{ pageId: string; blockId: string; content: string }> = [];
    const onReplacement = (event: Event) => {
      replacements.push((event as CustomEvent).detail);
    };
    window.addEventListener("grafium-block-content-replaced", onReplacement);
    setUndoCallback("page-1", cb);
    pushUndo({
      type: "update_block",
      pageId: "page-1",
      blockId: "block-1",
      beforeContent: "old block",
      afterContent: "improved block",
    });

    try {
      await expect(performUndo()).resolves.toBe(true);
      expect(mockUpdateBlock).toHaveBeenLastCalledWith("block-1", "old block");
      expect(replacements.at(-1)).toEqual({ pageId: "page-1", blockId: "block-1", content: "old block" });
      expect(cb).toHaveBeenCalledTimes(1);

      await expect(performRedo()).resolves.toBe(true);
      expect(mockUpdateBlock).toHaveBeenLastCalledWith("block-1", "improved block");
      expect(replacements.at(-1)).toEqual({ pageId: "page-1", blockId: "block-1", content: "improved block" });
      expect(cb).toHaveBeenCalledTimes(2);
    } finally {
      window.removeEventListener("grafium-block-content-replaced", onReplacement);
      removeUndoCallback("page-1");
    }
  });

  it("undoes and redoes a batch of block text changes as one action", async () => {
    mockUpdateBlock.mockResolvedValue(undefined);
    const cb = vi.fn();
    setUndoCallback("page-1", cb);
    pushUndo({
      type: "update_blocks",
      pageId: "page-1",
      changes: [
        { blockId: "block-1", beforeContent: "first", afterContent: "TODO first" },
        { blockId: "block-2", beforeContent: "second", afterContent: "TODO second" },
      ],
    });

    try {
      await expect(performUndo()).resolves.toBe(true);
      expect(mockUpdateBlock).toHaveBeenNthCalledWith(1, "block-1", "first");
      expect(mockUpdateBlock).toHaveBeenNthCalledWith(2, "block-2", "second");
      expect(cb).toHaveBeenCalledTimes(1);

      await expect(performRedo()).resolves.toBe(true);
      expect(mockUpdateBlock).toHaveBeenNthCalledWith(3, "block-1", "TODO first");
      expect(mockUpdateBlock).toHaveBeenNthCalledWith(4, "block-2", "TODO second");
      expect(cb).toHaveBeenCalledTimes(2);
    } finally {
      removeUndoCallback("page-1");
    }
  });

  it("undoes and redoes accepted link suggestions through their stored snapshots", async () => {
    mockUndoLinkCandidateAccept.mockResolvedValue({} as any);
    mockAcceptLinkCandidate.mockResolvedValue({} as any);
    const cb = vi.fn();
    setUndoCallback("page-1", cb);
    pushUndo({
      type: "accept_link_candidates",
      pageId: "page-1",
      candidateIds: ["candidate-1", "candidate-2"],
    });

    try {
      await expect(performUndo()).resolves.toBe(true);
      expect(mockUndoLinkCandidateAccept).toHaveBeenNthCalledWith(1, "candidate-2");
      expect(mockUndoLinkCandidateAccept).toHaveBeenNthCalledWith(2, "candidate-1");
      expect(cb).toHaveBeenCalledTimes(1);

      await expect(performRedo()).resolves.toBe(true);
      expect(mockAcceptLinkCandidate).toHaveBeenNthCalledWith(1, "candidate-1");
      expect(mockAcceptLinkCandidate).toHaveBeenNthCalledWith(2, "candidate-2");
      expect(cb).toHaveBeenCalledTimes(2);
    } finally {
      removeUndoCallback("page-1");
    }
  });

  it("undoes and redoes multi-block paste while preserving nested parent relationships", async () => {
    mockDeleteBlocks.mockResolvedValue([]);
    mockUpdateBlock.mockResolvedValue(undefined);
    mockCreateBlocks.mockResolvedValue([
      block({ id: "old-parent", content: "pasted parent", order_index: 1 }),
      block({ id: "old-child", parent_id: "old-parent", content: "pasted child", order_index: 0 }),
    ]);

    const cb = vi.fn();
    setUndoCallback("page-1", cb);
    pushUndo({
      type: "insert_blocks",
      pageId: "page-1",
      anchorBlockId: "anchor",
      beforeContent: "before paste",
      afterContent: "after paste",
      insertedBlocks: [
        block({ id: "old-parent", content: "pasted parent", order_index: 1 }),
        block({ id: "old-child", parent_id: "old-parent", content: "pasted child", order_index: 0 }),
      ],
    });

    try {
      await expect(performUndo()).resolves.toBe(true);
      expect(mockDeleteBlocks).toHaveBeenCalledWith("page-1", ["old-child", "old-parent"]);
      expect(mockUpdateBlock).toHaveBeenLastCalledWith("anchor", "before paste");
      expect(cb).toHaveBeenCalledTimes(1);

      await expect(performRedo()).resolves.toBe(true);
      expect(mockUpdateBlock).toHaveBeenLastCalledWith("anchor", "after paste");
      expect(mockCreateBlocks).toHaveBeenCalledWith("page-1", [
        {
          id: "old-parent",
          parentId: null,
          parentIndex: undefined,
          orderIndex: 1,
          content: "pasted parent",
          blockType: "text",
          properties: {},
        },
        {
          id: "old-child",
          parentId: undefined,
          parentIndex: 0,
          orderIndex: 0,
          content: "pasted child",
          blockType: "text",
          properties: {},
        },
      ]);
      expect((globalThis as any).__undoStack[0].insertedBlocks.map((b: Block) => b.id)).toEqual([
        "old-parent",
        "old-child",
      ]);
      expect(cb).toHaveBeenCalledTimes(2);
    } finally {
      removeUndoCallback("page-1");
    }
  });

  it("undoes and redoes deleted subtrees while preserving nested parent relationships", async () => {
    mockCreateBlocks.mockResolvedValue([
      block({ id: "old-parent", content: "deleted parent", order_index: 1 }),
      block({ id: "old-child", parent_id: "old-parent", content: "deleted child", order_index: 0 }),
    ]);
    mockDeleteBlocks.mockResolvedValue([
      block({ id: "old-child", parent_id: "old-parent", content: "deleted child", order_index: 0 }),
      block({ id: "old-parent", content: "deleted parent", order_index: 1 }),
    ]);

    const cb = vi.fn();
    setUndoCallback("page-1", cb);
    pushUndo({
      type: "delete_blocks",
      pageId: "page-1",
      blocks: [
        block({ id: "old-parent", content: "deleted parent", order_index: 1 }),
        block({ id: "old-child", parent_id: "old-parent", content: "deleted child", order_index: 0 }),
      ],
    });

    try {
      await expect(performUndo()).resolves.toBe(true);
      expect(mockCreateBlocks).toHaveBeenCalledWith("page-1", [
        {
          id: "old-parent",
          parentId: null,
          parentIndex: undefined,
          orderIndex: 1,
          content: "deleted parent",
          blockType: "text",
          properties: {},
        },
        {
          id: "old-child",
          parentId: undefined,
          parentIndex: 0,
          orderIndex: 0,
          content: "deleted child",
          blockType: "text",
          properties: {},
        },
      ]);
      expect(cb).toHaveBeenCalledTimes(1);

      await expect(performRedo()).resolves.toBe(true);
      expect(mockDeleteBlocks).toHaveBeenLastCalledWith("page-1", ["old-child", "old-parent"]);
      expect((globalThis as any).__undoStack[0].blocks.map((b: Block) => b.id)).toEqual([
        "old-child",
        "old-parent",
      ]);
      expect(cb).toHaveBeenCalledTimes(2);
    } finally {
      removeUndoCallback("page-1");
    }
  });

  it("keeps 50 app-level undo actions", () => {
    for (let i = 0; i < APP_UNDO_LIMIT + 1; i++) {
      pushUndo({
        type: "update_block",
        pageId: "page-1",
        blockId: `block-${i}`,
        beforeContent: `before ${i}`,
        afterContent: `after ${i}`,
      });
    }

    expect(APP_UNDO_LIMIT).toBe(50);
    expect(getUndoStackSize()).toBe(50);
  });

  it("retains a failed grouped undo and notifies every affected page", async () => {
    const cb1 = vi.fn();
    const cb2 = vi.fn();
    setUndoCallback("page-1", cb1);
    setUndoCallback("page-2", cb2);
    vi.mocked(listBlocks).mockRejectedValue(new Error("offline"));
    pushUndo({
      type: "delete_block_selection",
      pageId: "page-1",
      groups: [
        { pageId: "page-1", blocks: [block({ id: "a", content: "one" })] },
        { pageId: "page-2", blocks: [block({ id: "b", page_id: "page-2", content: "two" })] },
      ],
      placeholderIds: {},
    });
    try {
      await expect(performUndo()).resolves.toBe(false);
      expect(getUndoStackSize()).toBe(1);
      expect((globalThis as any).__redoStack).toHaveLength(0);
      expect(cb1).toHaveBeenCalledTimes(1);
      expect(cb2).toHaveBeenCalledTimes(1);
      expect(showToast).toHaveBeenCalledWith(expect.stringContaining("Undo again to retry"), "error");
    } finally {
      removeUndoCallback("page-1");
      removeUndoCallback("page-2");
    }
  });

  it("blocks overlapping undo and redo while an existing action is in flight", async () => {
    let finish!: () => void;
    mockUpdateBlock.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    pushUndo({
      type: "update_block",
      pageId: "page-1",
      blockId: "a",
      beforeContent: "before",
      afterContent: "after",
    });
    const undo = performUndo();
    await expect(performRedo()).resolves.toBe(false);
    await expect(performUndo()).resolves.toBe(false);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining("still running"), "info");
    finish();
    await expect(undo).resolves.toBe(true);
    expect(mockUpdateBlock).toHaveBeenCalledTimes(1);
  });
});
