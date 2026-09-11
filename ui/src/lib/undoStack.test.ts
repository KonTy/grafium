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
  undoLinkCandidateAccept: vi.fn(),
  updateBlock: vi.fn(),
}));

import {
  aiReapplySummaryInsert,
  aiUndoSummaryInsert,
  type SummaryWrapChange,
} from "./knowledge";
import {
  acceptLinkCandidate,
  createBlocks,
  deleteBlocks,
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
  });

  it("reverses the summary insert and calls the page's reload callback", async () => {
    mockUndo.mockResolvedValue(undefined);
    const cb = vi.fn();
    setUndoCallback("page-1", cb);

    const wrapChanges: SummaryWrapChange[] = [
      { blockId: "b-existing", previousContent: "before", newContent: "after" },
    ];
    pushUndo({
      type: "insert_summary",
      pageId: "page-1",
      insertedBlockId: "b-summary",
      insertedContent: "Summary body",
      insertedAfterBlockId: "b-anchor",
      wrapChanges,
    });

    try {
      const ok = await performUndo();
      expect(ok).toBe(true);
      expect(mockUndo).toHaveBeenCalledWith("b-summary", wrapChanges);
      expect(cb).toHaveBeenCalledTimes(1);
    } finally {
      removeUndoCallback("page-1");
    }
  });

  it("re-adds the action to the undo stack when the backend fails, so a follow-up Ctrl-Z isn't lost", async () => {
    mockUndo.mockRejectedValue(new Error("boom"));
    pushUndo({
      type: "insert_summary",
      pageId: "page-1",
      insertedBlockId: "b-summary",
      insertedContent: "Summary body",
      insertedAfterBlockId: null,
      wrapChanges: [],
    });

    const ok = await performUndo();
    expect(ok).toBe(false);
    expect((globalThis as any).__undoStack.length).toBe(1);
  });

  it("redo recreates the summary block and rebinds the undo entry to the fresh id", async () => {
    // Set up: pretend we already went summary-insert → undo, so the redo
    // stack has one entry.
    const wrapChanges: SummaryWrapChange[] = [
      { blockId: "b-existing", previousContent: "before", newContent: "after" },
    ];
    (globalThis as any).__redoStack = [
      {
        type: "insert_summary",
        pageId: "page-1",
        insertedBlockId: "b-summary-old",
        insertedContent: "Summary body",
        insertedAfterBlockId: "b-anchor",
        wrapChanges,
      },
    ];

    mockReapply.mockResolvedValue({
      insertedBlockId: "b-summary-new",
      insertedContent: "Summary body",
      insertedAfterBlockId: "b-anchor",
      wrapChanges,
    });

    const cb = vi.fn();
    setUndoCallback("page-1", cb);

    try {
      const ok = await performRedo();
      expect(ok).toBe(true);
      expect(mockReapply).toHaveBeenCalledWith("page-1", "Summary body", "b-anchor", wrapChanges);
      // The redo should have flipped the entry back onto the undo stack
      // with the *new* block id, so a follow-up Ctrl-Z targets what
      // reapply actually created (not the stale old id).
      const undoStack = (globalThis as any).__undoStack;
      expect(undoStack.length).toBe(1);
      expect(undoStack[0].insertedBlockId).toBe("b-summary-new");
      expect(cb).toHaveBeenCalledTimes(1);
    } finally {
      removeUndoCallback("page-1");
    }
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
});
