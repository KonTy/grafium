import type { Block, CreateBlockBatchItem } from "./api";
import {
  acceptLinkCandidate,
  createBlocks,
  deleteBlocks,
  undoLinkCandidateAccept,
  updateBlock,
} from "./api";
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
//   - `update_block` / `update_blocks`: a user-visible command changed block
//      text outside CodeMirror's own keystroke history; undo restores the
//      original text and redo reapplies the rewrite.
//   - `insert_blocks`: a structural insert/paste added blocks and optionally
//      changed an existing anchor block; undo removes the inserted blocks and
//      restores the anchor, redo recreates the blocks.
//   - `accept_link_candidates`: accepting reviewable link suggestions changed
//      one or more blocks; undo delegates to the candidate snapshot backend.
export interface BlockContentChange {
  blockId: string;
  beforeContent: string;
  afterContent: string;
}

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
    }
  | {
      type: "update_block";
      pageId: string;
      blockId: string;
      beforeContent: string;
      afterContent: string;
    }
  | {
      type: "update_blocks";
      pageId: string;
      changes: BlockContentChange[];
    }
  | {
      type: "insert_blocks";
      pageId: string;
      anchorBlockId: string | null;
      beforeContent: string | null;
      afterContent: string | null;
      insertedBlocks: Block[];
    }
  | {
      type: "accept_link_candidates";
      pageId: string;
      candidateIds: string[];
    };

// Store on window to guarantee single instance across all module imports
const w = globalThis as any;
if (!w.__undoStack) w.__undoStack = [];
if (!w.__redoStack) w.__redoStack = [];

function getUndoStack(): UndoAction[] { return w.__undoStack; }
function getRedoStack(): UndoAction[] { return w.__redoStack; }

export const APP_UNDO_LIMIT = 50;

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
    case "update_block":
      return `update_block block: ${action.blockId}`;
    case "update_blocks":
      return `update_blocks blocks: ${action.changes.length}`;
    case "insert_blocks":
      return `insert_blocks blocks: ${action.insertedBlocks.length} anchor: ${action.anchorBlockId ?? "none"}`;
    case "accept_link_candidates":
      return `accept_link_candidates candidates: ${action.candidateIds.length}`;
  }
}

function depthOfBlock(block: Block, byId: Map<string, Block>): number {
  let depth = 0;
  let parentId = block.parent_id;
  const seen = new Set<string>();
  while (parentId && !seen.has(parentId)) {
    seen.add(parentId);
    const parent = byId.get(parentId);
    if (!parent) break;
    depth += 1;
    parentId = parent.parent_id;
  }
  return depth;
}

function deepestBlocksFirst(blocks: readonly Block[]): Block[] {
  const byId = new Map(blocks.map((block) => [block.id, block]));
  return [...blocks].sort((a, b) => {
    const depthDelta = depthOfBlock(b, byId) - depthOfBlock(a, byId);
    return depthDelta || b.order_index - a.order_index;
  });
}

function shallowestBlocksFirst(blocks: readonly Block[]): Block[] {
  const byId = new Map(blocks.map((block) => [block.id, block]));
  return [...blocks].sort((a, b) => {
    const depthDelta = depthOfBlock(a, byId) - depthOfBlock(b, byId);
    return depthDelta || a.order_index - b.order_index;
  });
}

function batchItemsForInsertedBlocks(blocks: readonly Block[]): CreateBlockBatchItem[] {
  const insertedIndexById = new Map(blocks.map((block, index) => [block.id, index]));
  return blocks.map((block) => {
    const parentIndex = block.parent_id ? insertedIndexById.get(block.parent_id) : undefined;
    return {
      id: block.id,
      parentId: parentIndex === undefined ? block.parent_id : undefined,
      parentIndex,
      orderIndex: block.order_index,
      content: block.content,
      blockType: block.block_type,
      properties: block.properties,
    };
  });
}

export function pushUndo(action: UndoAction) {
  const stack = getUndoStack();
  stack.push(action);
  if (stack.length > APP_UNDO_LIMIT) stack.shift();
  getRedoStack().length = 0;
  console.log("[undoStack] PUSH:", actionSummary(action), "stack now:", stack.length);
}

export function removeUndoActions(predicate: (action: UndoAction) => boolean): number {
  const stack = getUndoStack();
  let removed = 0;
  for (let index = stack.length - 1; index >= 0; index -= 1) {
    if (predicate(stack[index])) {
      stack.splice(index, 1);
      removed += 1;
    }
  }
  return removed;
}

// Invoked internally when a redo pushes an "insert_summary" back onto
// the undo stack — same shape as `pushUndo` but without clearing the
// redo stack, so re-invoking Ctrl-Y after Ctrl-Z stays consistent.
function pushUndoWithoutClearingRedo(action: UndoAction) {
  const stack = getUndoStack();
  stack.push(action);
  if (stack.length > APP_UNDO_LIMIT) stack.shift();
  console.log("[undoStack] PUSH (redo-flip):", actionSummary(action), "stack now:", stack.length);
}

function pushRedo(action: UndoAction) {
  const stack = getRedoStack();
  stack.push(action);
  if (stack.length > APP_UNDO_LIMIT) stack.shift();
  console.log("[undoStack] REDO PUSH:", actionSummary(action), "stack now:", stack.length);
}

function notifyBlockContentReplaced(pageId: string, blockId: string, content: string) {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent("grafium-block-content-replaced", {
    detail: { pageId, blockId, content },
  }));
}

export async function performUndo(): Promise<boolean> {
  const stack = getUndoStack();
  const action = stack.pop();
  if (!action) return false;

  if (action.type === "delete_blocks") {
    let restoredBlocks: Block[];
    try {
      restoredBlocks = await createBlocks(
        action.pageId,
        batchItemsForInsertedBlocks(shallowestBlocksFirst(action.blocks))
      );
    } catch (e) {
      console.error("[undoStack] delete_blocks undo failed:", e);
      stack.push(action);
      return false;
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

  if (action.type === "update_block") {
    try {
      await updateBlock(action.blockId, action.beforeContent);
      notifyBlockContentReplaced(action.pageId, action.blockId, action.beforeContent);
    } catch (e) {
      console.error("[undoStack] update_block undo failed:", e);
      stack.push(action);
      return false;
    }
    pushRedo(action);
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(action);
    }
    return true;
  }

  if (action.type === "update_blocks") {
    try {
      for (const change of action.changes) {
        await updateBlock(change.blockId, change.beforeContent);
        notifyBlockContentReplaced(action.pageId, change.blockId, change.beforeContent);
      }
    } catch (e) {
      console.error("[undoStack] update_blocks undo failed:", e);
      stack.push(action);
      return false;
    }
    pushRedo(action);
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(action);
    }
    return true;
  }

  if (action.type === "insert_blocks") {
    try {
      await deleteBlocks(action.pageId, deepestBlocksFirst(action.insertedBlocks).map((block) => block.id));
      if (action.anchorBlockId && action.beforeContent !== null) {
        await updateBlock(action.anchorBlockId, action.beforeContent);
        notifyBlockContentReplaced(action.pageId, action.anchorBlockId, action.beforeContent);
      }
    } catch (e) {
      console.error("[undoStack] insert_blocks undo failed:", e);
      stack.push(action);
      return false;
    }
    pushRedo(action);
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(action);
    }
    return true;
  }

  if (action.type === "accept_link_candidates") {
    try {
      for (const candidateId of [...action.candidateIds].reverse()) {
        await undoLinkCandidateAccept(candidateId);
      }
    } catch (e) {
      console.error("[undoStack] accept_link_candidates undo failed:", e);
      stack.push(action);
      return false;
    }
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
    let deletedBlocks: Block[];
    try {
      deletedBlocks = await deleteBlocks(
        action.pageId,
        deepestBlocksFirst(action.blocks).map((block) => block.id)
      );
    } catch (e) {
      console.error("[undoStack] delete_blocks redo failed:", e);
      redoStack.push(action);
      return false;
    }
    const redoAction = { ...action, blocks: deletedBlocks.length ? deletedBlocks : action.blocks };
    pushUndoWithoutClearingRedo(redoAction);
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(redoAction);
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

  if (action.type === "update_block") {
    try {
      await updateBlock(action.blockId, action.afterContent);
      notifyBlockContentReplaced(action.pageId, action.blockId, action.afterContent);
    } catch (e) {
      console.error("[undoStack] update_block redo failed:", e);
      redoStack.push(action);
      return false;
    }
    pushUndoWithoutClearingRedo(action);
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(action);
    }
    return true;
  }

  if (action.type === "update_blocks") {
    try {
      for (const change of action.changes) {
        await updateBlock(change.blockId, change.afterContent);
        notifyBlockContentReplaced(action.pageId, change.blockId, change.afterContent);
      }
    } catch (e) {
      console.error("[undoStack] update_blocks redo failed:", e);
      redoStack.push(action);
      return false;
    }
    pushUndoWithoutClearingRedo(action);
    const cb = undoCallbacks.get(action.pageId);
    if (cb) {
      cb(action);
    }
    return true;
  }

  if (action.type === "insert_blocks") {
    try {
      if (action.anchorBlockId && action.afterContent !== null) {
        await updateBlock(action.anchorBlockId, action.afterContent);
        notifyBlockContentReplaced(action.pageId, action.anchorBlockId, action.afterContent);
      }

      const createdBlocks = await createBlocks(action.pageId, batchItemsForInsertedBlocks(action.insertedBlocks));
      pushUndoWithoutClearingRedo({
        ...action,
        insertedBlocks: createdBlocks,
      });
      const cb = undoCallbacks.get(action.pageId);
      if (cb) {
        cb({ ...action, insertedBlocks: createdBlocks });
      }
    } catch (e) {
      console.error("[undoStack] insert_blocks redo failed:", e);
      redoStack.push(action);
      return false;
    }
    return true;
  }

  if (action.type === "accept_link_candidates") {
    try {
      for (const candidateId of action.candidateIds) {
        await acceptLinkCandidate(candidateId);
      }
    } catch (e) {
      console.error("[undoStack] accept_link_candidates redo failed:", e);
      redoStack.push(action);
      return false;
    }
    pushUndoWithoutClearingRedo(action);
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
