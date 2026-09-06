import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the tauri command wrappers used by the insert_summary undo path so
// tests never touch the real invoke() bridge.
vi.mock("./knowledge", () => ({
  aiUndoSummaryInsert: vi.fn(),
  aiReapplySummaryInsert: vi.fn(),
}));

vi.mock("./api", () => ({
  createBlock: vi.fn(),
  deleteBlock: vi.fn(),
}));

import {
  aiReapplySummaryInsert,
  aiUndoSummaryInsert,
  type SummaryWrapChange,
} from "./knowledge";
import {
  performRedo,
  performUndo,
  pushUndo,
  setUndoCallback,
  removeUndoCallback,
} from "./undoStack";

const mockUndo = vi.mocked(aiUndoSummaryInsert);
const mockReapply = vi.mocked(aiReapplySummaryInsert);

describe("undoStack — insert_summary flow", () => {
  beforeEach(() => {
    (globalThis as any).__undoStack = [];
    (globalThis as any).__redoStack = [];
    mockUndo.mockReset();
    mockReapply.mockReset();
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
});
