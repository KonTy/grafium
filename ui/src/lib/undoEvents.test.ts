import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  createBlocks: vi.fn(),
  deleteBlocks: vi.fn(),
}));

import { createBlocks, deleteBlocks } from "./api";
import { attachAppUndoRedoListeners } from "./undoEvents";
import {
  pushUndo,
  removeUndoCallback,
  setUndoCallback,
} from "./undoStack";

const mockCreateBlocks = vi.mocked(createBlocks);
const mockDeleteBlocks = vi.mocked(deleteBlocks);

function flushAsyncWork(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe("app undo event dispatch", () => {
  beforeEach(() => {
    (globalThis as any).__undoStack = [];
    (globalThis as any).__redoStack = [];
    mockCreateBlocks.mockReset();
    mockDeleteBlocks.mockReset();
  });

  it("dispatches a single undo event to only the targeted page callback once", async () => {
    const restoredBlock = {
      id: "restored-block",
      page_id: "page-a",
      parent_id: null,
      order_index: 0,
      content: "restored",
      block_type: "text",
      properties: {},
      created_at: "0",
      updated_at: "0",
    };
    mockCreateBlocks.mockResolvedValue([restoredBlock]);
    mockDeleteBlocks.mockResolvedValue([restoredBlock]);

    const pageACallback = vi.fn();
    const pageBCallback = vi.fn();
    setUndoCallback("page-a", pageACallback);
    setUndoCallback("page-b", pageBCallback);

    pushUndo({
      type: "delete_blocks",
      pageId: "page-a",
      blocks: [restoredBlock],
    });

    const target = new EventTarget();
    const detach = attachAppUndoRedoListeners(target);

    try {
      target.dispatchEvent(new Event("app-undo"));
      await flushAsyncWork();

      expect(pageACallback).toHaveBeenCalledTimes(1);
      expect(pageACallback).toHaveBeenCalledWith({
        type: "delete_blocks", pageId: "page-a", blocks: [restoredBlock],
      });
      expect(pageBCallback).not.toHaveBeenCalled();
      expect(mockCreateBlocks).toHaveBeenCalledTimes(1);
      expect(mockCreateBlocks).toHaveBeenCalledWith("page-a", [
        expect.objectContaining({ id: restoredBlock.id, content: restoredBlock.content }),
      ]);

      target.dispatchEvent(new Event("app-redo"));
      await flushAsyncWork();

      expect(mockDeleteBlocks).toHaveBeenCalledTimes(1);
      expect(mockDeleteBlocks).toHaveBeenCalledWith("page-a", [restoredBlock.id]);
      expect(pageACallback).toHaveBeenCalledTimes(2);
      expect(pageBCallback).not.toHaveBeenCalled();

      detach();
      target.dispatchEvent(new Event("app-undo"));
      target.dispatchEvent(new Event("app-redo"));
      await flushAsyncWork();
      expect(mockCreateBlocks).toHaveBeenCalledTimes(1);
      expect(mockDeleteBlocks).toHaveBeenCalledTimes(1);
      expect(pageACallback).toHaveBeenCalledTimes(2);
    } finally {
      detach();
      removeUndoCallback("page-a");
      removeUndoCallback("page-b");
    }
  });
});
