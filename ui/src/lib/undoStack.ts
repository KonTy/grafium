import type { Block } from "./api";
import { createBlock, deleteBlock } from "./api";
import {
  aiUndoSummaryInsert,
  aiReapplySummaryInsert,
  type SummaryWrapChange,
} from "./knowledge";

// The undo stack tracks reversible top-level actions the user did to the
// graph — not per-keystroke edits (CodeMirror handles those inside a
// single block). Currently:
//   - `delete_blocks`: user (or a delete-blocks flow) removed one or
//      more blocks; undo recreates them, redo deletes again.
//   - `insert_summary`: user pressed "Insert into page" on an AI
//      summary; undo deletes the freshly-created summary block AND
//      restores each block whose text was rewrapped with `[[wiki-link]]`s
//      during the same operation; redo recreates the summary block and
//      reapplies the wraps.
export type UndoAction =
  | {
      type: "delete_blocks";
      blocks: Block[];
      pageId: string;
    }
  | {
      type: "insert_summary";
      pageId: string;
      insertedBlockId: string;
      insertedContent: string;
      insertedAfterBlockId: string | null;
      wrapChanges: SummaryWrapChange[];
    };

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

// Cheap per-action label for the debug log — helps diagnose "why did
// Ctrl-Z do X and not Y" without pretty-printing the whole payload.
function actionSummary(action: UndoAction): string {
  switch (action.type) {
    case "delete_blocks":
      return `delete_blocks blocks: ${action.blocks.length}`;
    case "insert_summary":
      return `insert_summary block: ${action.insertedBlockId} wraps: ${action.wrapChanges.length}`;
  }
}

export function pushUndo(action: UndoAction) {
  const stack = getUndoStack();
  stack.push(action);
  if (stack.length > MAX_UNDO) stack.shift();
  getRedoStack().length = 0;
  console.log("[undoStack] PUSH:", actionSummary(action), "stack now:", stack.length);
}

// Invoked internally when a redo pushes an "insert_summary" back onto
// the undo stack — same shape as `pushUndo` but without clearing the
// redo stack, so re-invoking Ctrl-Y after Ctrl-Z stays consistent.
function pushUndoWithoutClearingRedo(action: UndoAction) {
  const stack = getUndoStack();
  stack.push(action);
  if (stack.length > MAX_UNDO) stack.shift();
  console.log("[undoStack] PUSH (redo-flip):", actionSummary(action), "stack now:", stack.length);
}

function pushRedo(action: UndoAction) {
  const stack = getRedoStack();
  stack.push(action);
  if (stack.length > MAX_UNDO) stack.shift();
  console.log("[undoStack] REDO PUSH:", actionSummary(action), "stack now:", stack.length);
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
    pushRedo({
      type: "delete_blocks",
      blocks: restoredBlocks,
      pageId: action.pageId,
    });
    // Notify the correct PageContent instance by pageId
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb({ ...action, blocks: restoredBlocks });
    }
    return true;
  }

  if (action.type === "insert_summary") {
    try {
      await aiUndoSummaryInsert(action.insertedBlockId, action.wrapChanges);
    } catch (e) {
      console.error("[undoStack] insert_summary undo failed:", e);
      // Put the action back so a follow-up Ctrl-Z isn't lost silently.
      stack.push(action);
      return false;
    }
    // Redo is the *same* action shape — it carries everything needed
    // to recreate the summary block and re-apply the wraps. Note that
    // after redo runs, the new inserted block id will differ; the
    // redo handler pushes a fresh insert_summary onto the undo stack
    // with that new id.
    pushRedo(action);
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(action);
    }
    return true;
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
    return true;
  }

  if (action.type === "insert_summary") {
    try {
      const result = await aiReapplySummaryInsert(
        action.pageId,
        action.insertedContent,
        action.insertedAfterBlockId,
        action.wrapChanges,
      );
      // The reapply creates a *new* block with a new id, so the next
      // undo needs to target that id, not the old one.
      pushUndoWithoutClearingRedo({
        type: "insert_summary",
        pageId: action.pageId,
        insertedBlockId: result.insertedBlockId,
        insertedContent: result.insertedContent,
        insertedAfterBlockId: result.insertedAfterBlockId,
        wrapChanges: result.wrapChanges,
      });
    } catch (e) {
      console.error("[undoStack] insert_summary redo failed:", e);
      redoStack.push(action);
      return false;
    }
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(action);
    }
    return true;
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
