import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  aiGenerateReferences,
  aiInsertPageSummary,
  aiReapplySummaryInsert,
  aiUndoSummaryInsert,
  type AiInsertSummaryResult,
} from "./knowledge";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

beforeEach(() => vi.mocked(invoke).mockReset());

it("passes the captured source snapshot and explicit wrapping choice to native insertion", async () => {
  const guards = { graphPath: "synthetic-graph", expectedBlocks: [], wrapExisting: true };
  await aiInsertPageSummary("source-page", "Answer", [], "source-anchor", guards);
  expect(invoke).toHaveBeenCalledWith("ai_insert_page_summary", {
    pageId: "source-page", titleAnswer: "Answer", topics: [], afterBlockId: "source-anchor", ...guards,
  });
});

it("passes generation identity and source guards without requiring embeddings", async () => {
  const guards = { graphPath: "synthetic-graph", expectedBlocks: [] };
  await aiGenerateReferences("source-page", "operation", guards);
  expect(invoke).toHaveBeenCalledWith("ai_generate_references", {
    pageId: "source-page", operationId: "operation", ...guards,
  });
});

it("round trips the complete tree receipt to undo and redo", async () => {
  const receipt: AiInsertSummaryResult = {
    graphPath: "synthetic-graph",
    pageId: "source-page",
    insertedBlockId: "root",
    insertedContent: "Summary",
    insertedAfterBlockId: null,
    insertedBlocks: [{
      id: "root", page_id: "source-page", parent_id: null, order_index: 0,
      content: "Summary", block_type: "Text", properties: {}, created_at: 42, updated_at: 42,
    }],
    siblingOrderBefore: [],
    resolvedTargets: [],
    createdTargets: [{
      id: "concept", title: "New concept", file_path: null, created_at: 42, updated_at: 42,
      is_journal: false, properties: {},
    }],
    unlinkedTargets: [{
      sourcePhrase: "Ambiguous concept", targetTitle: "Ambiguous concept", reason: "Multiple approved aliases match.",
    }],
    wrapChanges: [],
  };
  const undoResult = { retainedTargets: [{ pageId: "concept", title: "New concept", reason: "Used elsewhere" }] };
  vi.mocked(invoke).mockResolvedValueOnce(undoResult).mockResolvedValueOnce(receipt);
  expect(await aiUndoSummaryInsert(receipt)).toEqual(undoResult);
  expect(invoke).toHaveBeenLastCalledWith("ai_undo_summary_insert", { receipt });
  expect(await aiReapplySummaryInsert(receipt)).toEqual(receipt);
  expect(invoke).toHaveBeenLastCalledWith("ai_reapply_summary_insert", { receipt });
});
