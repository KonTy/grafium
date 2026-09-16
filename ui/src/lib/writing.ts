import { invoke } from "@tauri-apps/api/core";
import { getGraphInfo, getPage, listBlocks, type Block } from "./api";
import type { CurrentBlockAnchor } from "./currentBlockAnchor";
import { isFencedCodeBlock } from "./codeFence";
import { flushPageEditors } from "./editorPersistence";
import type { WritingRewriteAction } from "./undoStack";

export type WritingScope = "block" | "page";
export interface WritingInputBlock { id: string; content: string }
export interface WritingFinding { label: string; detail: string; quote: string }
export interface WritingAnalysis {
  score: number | null;
  summary: string;
  findings: WritingFinding[];
  wordCount: number;
  analyzedWordCount: number;
  chunksAnalyzed: number;
  chunksTotal: number;
}
export interface WritingContentChange {
  blockId: string;
  beforeContent: string;
  afterContent: string;
}
export interface WritingRewriteIssue {
  blockId: string;
  blockOrdinal: number;
  lineOrdinal: number | null;
  reason: string;
}
export interface WritingRewriteResult {
  blocks: WritingInputBlock[];
  skipped: WritingRewriteIssue[];
}
export interface WritingRewrittenDetail {
  action: WritingRewriteAction;
  undo: boolean;
  pageId: string;
  changes: WritingContentChange[];
  markUndoBoundary: boolean;
  refreshes: Promise<void>[];
}
export interface WritingContext {
  pageId: string;
  anchor: CurrentBlockAnchor | null;
  preferFocusedPageForPageScope: boolean;
}
export interface WritingTarget {
  graphPath: string;
  pageId: string;
  pageTitle: string;
  scope: WritingScope;
  blocks: WritingInputBlock[];
  snapshot: Block[];
  skippedBlocks: number;
}

export function writingTargetPageId(context: WritingContext): string {
  return context.preferFocusedPageForPageScope && context.anchor?.pageId
    ? context.anchor.pageId : context.pageId;
}

export function isWritingBlock(block: Block): boolean {
  return block.block_type.toLowerCase() === "text" && !!block.content.trim()
    && !isFencedCodeBlock(block.content) && !block.content.trimStart().startsWith("{{query");
}

export async function captureWritingTarget(context: WritingContext, scope: WritingScope): Promise<WritingTarget> {
  const pageId = writingTargetPageId(context);
  if (!pageId) throw new Error("Open a page or select a journal day first.");
  const blockId = context.anchor?.pageId === pageId ? context.anchor.blockId : null;
  if (scope === "block" && !blockId) throw new Error("Click a block in the editor first.");
  const graph = await getGraphInfo();
  await flushPageEditors(pageId);
  const page = await getPage({ id: pageId });
  if (context.preferFocusedPageForPageScope && !page.is_journal) {
    throw new Error("Select a block in the journal day you want to analyze.");
  }
  const snapshot = await listBlocks(pageId);
  const selected = scope === "page" ? snapshot : snapshot.filter((block) => block.id === blockId);
  if (!selected.length) throw new Error("The selected block or page is no longer available.");
  const blocks = selected.filter(isWritingBlock).map(({ id, content }) => ({ id, content }));
  if (!blocks.length) throw new Error("No eligible text to analyze. Code, queries, media and flashcards are left unchanged.");
  if ((await getGraphInfo()).path !== graph.path) throw new Error("The graph changed. Please try again.");
  return {
    graphPath: graph.path, pageId, pageTitle: page.title, scope, blocks, snapshot,
    skippedBlocks: selected.length - blocks.length,
  };
}

export function writingChanges(original: WritingInputBlock[], rewritten: WritingInputBlock[]): WritingContentChange[] {
  if (original.length !== rewritten.length) throw new Error("The model returned an incomplete rewrite. Nothing was changed.");
  const changes: WritingContentChange[] = [];
  for (let index = 0; index < original.length; index++) {
    const before = original[index];
    const after = rewritten[index];
    if (after.id !== before.id || typeof after.content !== "string" || !after.content.trim()) {
      throw new Error("The model changed the block structure or returned empty text. Nothing was changed.");
    }
    if (before.content !== after.content) {
      changes.push({ blockId: before.id, beforeContent: before.content, afterContent: after.content });
    }
  }
  return changes;
}

export function analyzeWriting(blocks: WritingInputBlock[], operationId: string): Promise<WritingAnalysis> {
  return invoke("ai_analyze_writing", { blocks, operationId });
}
export function rewriteWriting(blocks: WritingInputBlock[], operationId: string): Promise<WritingRewriteResult> {
  return invoke("ai_rewrite_writing", { blocks, operationId });
}
export function cancelWriting(operationId: string): Promise<void> {
  return invoke("ai_cancel_writing", { operationId });
}
export function applyWritingChanges(
  graphPath: string, pageId: string, changes: WritingContentChange[], expectedBlocks?: Block[],
): Promise<void> {
  return invoke("apply_writing_changes", { graphPath, pageId, changes, expectedBlocks });
}
