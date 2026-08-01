import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  createBlock: vi.fn(),
  deleteBlock: vi.fn(),
}));

import { createBlock } from "./api";
import { attachAppUndoRedoListeners } from "./undoEvents";
import {
  pushUndo,
  removeUndoCallback,
  setUndoCallback,
} from "./undoStack";

const mockCreateBlock = vi.mocked(createBlock);

function flushAsyncWork(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe("app undo event dispatch", () => {
  beforeEach(() => {
    (globalThis as any).__undoStack = [];
    (globalThis as any).__redoStack = [];
    mockCreateBlock.mockReset();
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
    mockCreateBlock.mockResolvedValue(restoredBlock);

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
      expect(pageBCallback).not.toHaveBeenCalled();
    } finally {
      detach();
      removeUndoCallback("page-a");
      removeUndoCallback("page-b");
    }
  });
});
