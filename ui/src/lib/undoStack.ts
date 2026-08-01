import type { Block } from "./api";
import { createBlock, deleteBlock } from "./api";

export interface UndoAction {
  type: "delete_blocks";
  blocks: Block[];
  pageId: string;
}

// Store on window to guarantee single instance across all module imports
const w = globalThis as any;
if (!w.__undoStack) w.__undoStack = [];
if (!w.__redoStack) w.__redoStack = [];

function getUndoStack(): UndoAction[] { return w.__undoStack; }
function getRedoStack(): UndoAction[] { return w.__redoStack; }

const MAX_UNDO = 50;

// Per-page callbacks so journal view (multiple PageContent instances) works
const undoCallbacks: Map<string, (action: UndoAction) => void> = new Map();

export function setUndoCallback(pageId: string, cb: (action: UndoAction) => void) {
  undoCallbacks.set(pageId, cb);
}

export function removeUndoCallback(pageId: string) {
  undoCallbacks.delete(pageId);
}

export function pushUndo(action: UndoAction) {
  const stack = getUndoStack();
  stack.push(action);
  if (stack.length > MAX_UNDO) stack.shift();
  getRedoStack().length = 0;
  console.log("[undoStack] PUSH:", action.type, "blocks:", action.blocks.length, "stack now:", stack.length);
}

export async function performUndo(): Promise<boolean> {
  const stack = getUndoStack();
  const action = stack.pop();
  if (!action) return false;

  if (action.type === "delete_blocks") {
    const restoredBlocks: Block[] = [];
    for (const block of action.blocks) {
      try {
        const restored = await createBlock(
          block.page_id,
          block.parent_id,
          block.order_index,
          block.content,
          block.block_type,
          block.properties
        );
        restoredBlocks.push(restored);
      } catch (e) {
        console.error("[undoStack] failed to restore block:", e);
      }
    }
    getRedoStack().push({
      type: "delete_blocks",
      blocks: restoredBlocks,
      pageId: action.pageId,
    });
    // Notify the correct PageContent instance by pageId
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb({ ...action, blocks: restoredBlocks });
    }
  }

  return true;
}

export async function performRedo(): Promise<boolean> {
  const redoStack = getRedoStack();
  const action = redoStack.pop();
  if (!action) return false;

  if (action.type === "delete_blocks") {
    for (const block of action.blocks) {
      await deleteBlock(block.id);
    }
    getUndoStack().push(action);
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(action);
    }
  }

  return true;
}

export function canUndo(): boolean {
  return getUndoStack().length > 0;
}

export function canRedo(): boolean {
  return getRedoStack().length > 0;
}

export function getUndoStackSize(): number {
  return getUndoStack().length;
}
