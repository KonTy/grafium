/**
 * Applying an {@link EditPlan} to the graph.
 *
 * Split from the planner so the risky half — the half that actually writes —
 * is small, explicit, and testable without a running backend. Every dependency
 * is injected through {@link ApplyDeps}; the default binding is the real API.
 *
 * Invariants this module keeps:
 *
 * - It is only ever called after the user clicks Apply on a preview card.
 * - Blocks are **appended**. Nothing existing is overwritten except by an
 *   explicit `replace_block`, which goes through `applyWritingChanges` with the
 *   captured snapshot so a concurrent edit aborts the write instead of losing it.
 * - Every write pushes an undo entry, one per page touched, so Ctrl+Z is always
 *   an honest escape hatch.
 */

import {
  createBlocks as apiCreateBlocks,
  createPage as apiCreatePage,
  getPage as apiGetPage,
  listBlocks as apiListBlocks,
  type Block,
  type CreateBlockBatchItem,
  type Page,
} from "./api";
import { applyWritingChanges as apiApplyWritingChanges } from "./writing";
import { pushUndo as apiPushUndo, type UndoAction } from "./undoStack";
import { actionBlocks, renderTags, type EditAction, type EditPlan } from "./aiActions";

export interface BlockTarget {
  graphPath: string;
  pageId: string;
  blockId: string;
  content: string;
  snapshot?: Block[];
}

export interface ApplyDeps {
  getPage: (opts: { id?: string; title?: string }) => Promise<Page>;
  createPage: (title: string, isJournal?: boolean) => Promise<Page>;
  listBlocks: (pageId: string) => Promise<Block[]>;
  createBlocks: (pageId: string, blocks: CreateBlockBatchItem[]) => Promise<Block[]>;
  applyWritingChanges: (
    graphPath: string,
    pageId: string,
    changes: { blockId: string; beforeContent: string; afterContent: string }[],
    expectedBlocks?: Block[],
  ) => Promise<void>;
  pushUndo: (action: UndoAction) => void;
}

export const defaultApplyDeps: ApplyDeps = {
  getPage: apiGetPage,
  createPage: apiCreatePage,
  listBlocks: apiListBlocks,
  createBlocks: apiCreateBlocks,
  applyWritingChanges: apiApplyWritingChanges,
  pushUndo: apiPushUndo,
};

export interface AppliedAction {
  action: EditAction;
  pageId: string;
  pageTitle: string;
  pageCreated: boolean;
  blockCount: number;
}

export interface ApplyPlanResult {
  applied: AppliedAction[];
  /** Pages the user asked to scan for connections, for the caller to kick off. */
  findLinks: { pageId: string; title: string }[];
  /** Undo entries pushed, so the notice can be honest about how many Ctrl+Z it takes. */
  undoSteps: number;
  /** Per-action failures. A bad action must not discard the ones that worked. */
  errors: string[];
}

/**
 * Find a page by title, creating it when missing.
 *
 * `create_page` is idempotent in the backend, but a lookup first is still worth
 * it: knowing whether the page already existed is the difference between
 * "Created Supplements" and "Added to Supplements" in the result notice.
 */
async function resolvePage(
  deps: ApplyDeps,
  title: string,
  isJournal: boolean,
): Promise<{ page: Page; created: boolean }> {
  try {
    return { page: await deps.getPage({ title }), created: false };
  } catch {
    return { page: await deps.createPage(title, isJournal), created: true };
  }
}

/** Next free top-level slot, so an append never lands on top of existing notes. */
function nextOrderIndex(blocks: Block[]): number {
  const roots = blocks.filter((block) => !block.parent_id);
  return roots.reduce((max, block) => Math.max(max, block.order_index + 1), 0);
}

function batchFor(action: EditAction, startIndex: number): CreateBlockBatchItem[] {
  const { parent, children } = actionBlocks(action);
  const items: CreateBlockBatchItem[] = [{ orderIndex: startIndex, content: parent }];
  children.forEach((content, index) => {
    items.push({ parentIndex: 0, orderIndex: index, content });
  });
  return items;
}

async function appendToPage(
  deps: ApplyDeps,
  action: EditAction,
  title: string,
  isJournal: boolean,
  result: ApplyPlanResult,
): Promise<void> {
  const { page, created } = await resolvePage(deps, title, isJournal);
  const existing = await deps.listBlocks(page.id);
  const inserted = await deps.createBlocks(page.id, batchFor(action, nextOrderIndex(existing)));
  deps.pushUndo({
    type: "insert_blocks",
    pageId: page.id,
    anchorBlockId: null,
    beforeContent: null,
    afterContent: null,
    insertedBlocks: inserted,
  });
  result.undoSteps += 1;
  result.applied.push({
    action,
    pageId: page.id,
    pageTitle: page.title,
    pageCreated: created,
    blockCount: inserted.length,
  });
  if (action.findLinks) result.findLinks.push({ pageId: page.id, title: page.title });
}

async function applyAction(
  deps: ApplyDeps,
  action: EditAction,
  blockTarget: BlockTarget | null,
  result: ApplyPlanResult,
): Promise<void> {
  switch (action.type) {
    case "append_to_journal":
    case "create_task":
      await appendToPage(deps, action, action.date!, true, result);
      return;

    case "append_to_page":
    case "create_page":
      await appendToPage(deps, action, action.page!, false, result);
      return;

    case "add_tags": {
      // Tagging is appending real wiki links: that is what makes the page show
      // up in the tag's backlinks, which is what people mean by "file it under".
      const tagged: EditAction = { ...action, title: renderTags(action.tags), content: "", tags: [] };
      await appendToPage(deps, tagged, action.page!, false, result);
      return;
    }

    case "replace_block": {
      if (!blockTarget) throw new Error("This conversation is not attached to a block.");
      const changes = [
        { blockId: blockTarget.blockId, beforeContent: blockTarget.content, afterContent: action.content.trim() },
      ];
      await deps.applyWritingChanges(blockTarget.graphPath, blockTarget.pageId, changes, blockTarget.snapshot);
      deps.pushUndo({
        type: "rewrite_writing",
        graphPath: blockTarget.graphPath,
        pageId: blockTarget.pageId,
        changes,
      });
      result.undoSteps += 1;
      result.applied.push({
        action,
        pageId: blockTarget.pageId,
        pageTitle: "",
        pageCreated: false,
        blockCount: 1,
      });
      return;
    }

    case "find_links": {
      const { page } = await resolvePage(deps, action.page!, false);
      result.findLinks.push({ pageId: page.id, title: page.title });
      result.applied.push({ action, pageId: page.id, pageTitle: page.title, pageCreated: false, blockCount: 0 });
      return;
    }
  }
}

/**
 * Apply every action in a plan.
 *
 * Actions run in order and a failure is recorded rather than thrown: when a
 * three-step plan trips on step two, silently discarding step one's work would
 * leave the user with no idea what state their notes are in.
 */
export async function applyEditPlan(
  plan: EditPlan,
  options: { blockTarget?: BlockTarget | null; deps?: ApplyDeps } = {},
): Promise<ApplyPlanResult> {
  const deps = options.deps ?? defaultApplyDeps;
  const result: ApplyPlanResult = { applied: [], findLinks: [], undoSteps: 0, errors: [] };

  for (const action of plan.actions) {
    try {
      await applyAction(deps, action, options.blockTarget ?? null, result);
    } catch (cause) {
      result.errors.push(`${action.type}: ${cause instanceof Error ? cause.message : String(cause)}`);
    }
  }
  return result;
}

/** Human summary of what just happened, including how many undo steps it takes. */
export function summarizeApplyResult(result: ApplyPlanResult): string {
  const parts: string[] = [];
  for (const entry of result.applied) {
    if (entry.action.type === "find_links") continue;
    if (entry.action.type === "replace_block") {
      parts.push("Replaced the captured block");
      continue;
    }
    const verb = entry.pageCreated ? "Created" : "Added to";
    parts.push(`${verb} ${entry.pageTitle}`);
  }
  if (result.findLinks.length) {
    parts.push(`looking for links on ${result.findLinks.map((target) => target.title).join(", ")}`);
  }
  if (!parts.length && !result.errors.length) return "Nothing to apply.";

  const undo =
    result.undoSteps > 1
      ? ` Ctrl+Z undoes this (${result.undoSteps} steps).`
      : result.undoSteps === 1
        ? " Ctrl+Z undoes this."
        : "";
  const failures = result.errors.length ? ` Failed: ${result.errors.join("; ")}.` : "";
  return `${parts.join(". ")}.${undo}${failures}`;
}
