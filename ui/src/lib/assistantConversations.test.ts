import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ResearchStreamHandlers } from "./research";
import type { AssistantRequest } from "./assistant";
import {
  getGlobalConversation, getSourceConversation, getAssistantConversation, sendAssistantQuestion,
  assistantConversationRunning, stopAssistantConversation, newAssistantConversation, stopAllAssistantConversations,
} from "./assistantConversations";

const mocks = vi.hoisted(() => ({ graph: vi.fn(), flush: vi.fn(), flushAll: vi.fn(), chat: vi.fn(), cancel: vi.fn() }));
vi.mock("./api", () => ({ getGraphInfo: mocks.graph }));
vi.mock("./editorPersistence", () => ({ flushPageEditors: mocks.flush, flushAllPageEditors: mocks.flushAll }));
vi.mock("./assistant", async (original) => ({
  ...await original<typeof import("./assistant")>(), assistantChat: mocks.chat, assistantCancel: mocks.cancel,
}));

let graphNumber = 0;
let graphPath: string;
beforeEach(() => {
  vi.clearAllMocks();
  graphPath = `/synthetic/assistant-${++graphNumber}`;
  mocks.graph.mockResolvedValue({ path: graphPath });
  mocks.flush.mockResolvedValue(undefined);
  mocks.flushAll.mockResolvedValue(undefined);
  mocks.cancel.mockResolvedValue(undefined);
  mocks.chat.mockImplementation(async (_request: AssistantRequest, handlers: ResearchStreamHandlers) => {
    handlers.onChunk("A complete answer.");
    handlers.onDone();
  });
});

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("one conversation lifecycle for both placements", () => {
  it("defaults global to no notes and sources to the current page or whole book, with no web", () => {
    expect(getGlobalConversation(graphPath)).toMatchObject({ context: { kind: "none" }, mode: "answer" });
    expect(getSourceConversation(graphPath, "day", "2026-09-16")).toMatchObject({
      context: { kind: "page", pageId: "day" }, contextLabel: "2026-09-16", mode: "answer",
    });
    expect(getSourceConversation(graphPath, "book", "My book", true).context).toEqual({ kind: "book", pageId: "book" });
  });

  it("expanding uses the identical object, while pages and graphs keep independent drafts", () => {
    const source = getSourceConversation(graphPath, "page", "Original title");
    source.draft = "Keep my source question";
    expect(getAssistantConversation(source.id)).toBe(source);
    expect(getSourceConversation(graphPath, "page", "Renamed")).toBe(source);
    expect(source.contextLabel).toBe("Renamed");
    expect(getGlobalConversation(graphPath)).not.toBe(source);
    expect(getSourceConversation(graphPath, "other", "Other").draft).toBe("");
    expect(getSourceConversation("/another/graph", "page", "Original title").draft).toBe("");
  });

  it("captures context, selected words, mode and question before editor persistence", async () => {
    const saving = deferred();
    mocks.flush.mockReturnValue(saving.promise);
    const source = getSourceConversation(graphPath, "page", "Book");
    source.context = { kind: "selection", pageId: "page", selection: { blockIds: ["b1"], text: "Selected text" } };
    source.contextLabel = "Selected passage";
    source.draft = "Explain";
    const pending = sendAssistantQuestion(source);
    await vi.waitFor(() => expect(mocks.flush).toHaveBeenCalledWith("page"));
    source.context.selection.text = "Different selection";
    source.context.selection.blockIds.push("b2");
    source.contextLabel = "Elsewhere";
    source.mode = "deep";
    source.draft = "Another draft";
    saving.resolve();
    await pending;
    expect(mocks.chat.mock.calls[0][0]).toMatchObject({
      question: "Explain", mode: "answer", context: {
        kind: "selection", pageId: "page", selection: { blockIds: ["b1"], text: "Selected text" },
      },
    });
    expect(source.messages[1]).toMatchObject({ contextLabel: "Selected passage", mode: "answer" });
    expect(source.draft).toBe("Another draft");
  });

  it.each(["graph", "book"] as const)("flushes mounted editors for %s without widening the native context", async (kind) => {
    const source = getSourceConversation(graphPath, "page", "Book");
    source.context = kind === "graph" ? { kind } : { kind, pageId: "page" };
    source.draft = "Question";
    await sendAssistantQuestion(source);
    expect(mocks.flushAll).toHaveBeenCalledOnce();
    expect(mocks.flush).not.toHaveBeenCalled();
    expect(mocks.chat.mock.calls[0][0].context).toEqual(source.context);
  });

  it("does not save/read note content in no-notes mode or expose note-backed history", async () => {
    const source = getSourceConversation(graphPath, "page", "Book");
    source.draft = "Question about private notes";
    await sendAssistantQuestion(source);
    source.context = { kind: "none" };
    source.contextLabel = "No notes";
    source.draft = "General question";
    mocks.flush.mockClear();
    await sendAssistantQuestion(source);
    expect(mocks.flush).not.toHaveBeenCalled();
    expect(mocks.flushAll).not.toHaveBeenCalled();
    expect(mocks.chat.mock.calls[1][0].history).toEqual([]);
    expect(source.messages).toHaveLength(4);
    source.draft = "General follow-up";
    await sendAssistantQuestion(source);
    expect(mocks.chat.mock.calls[2][0].history).toEqual([
      { role: "user", content: "General question" }, { role: "assistant", content: "A complete answer." },
    ]);
  });

  it("keeps a pending run alive without a mounted view and isolates other sources", async () => {
    const pending = deferred();
    let events!: ResearchStreamHandlers;
    mocks.chat.mockImplementationOnce((_request, handlers) => { events = handlers; return pending.promise; });
    const source = getSourceConversation(graphPath, "a", "A");
    source.draft = "First question";
    const sending = sendAssistantQuestion(source);
    await vi.waitFor(() => expect(events).toBeDefined());
    const expanded = getAssistantConversation(source.id)!;
    expect(assistantConversationRunning(expanded)).toBe(true);
    expanded.draft = "Ignored duplicate send";
    await sendAssistantQuestion(expanded);
    expect(mocks.chat).toHaveBeenCalledOnce();
    const other = getSourceConversation(graphPath, "b", "B");
    other.draft = "Other question";
    await sendAssistantQuestion(other);
    events.onChunk("Finished in background");
    events.onDone();
    pending.resolve();
    await sending;
    expect(expanded.messages[1].content).toBe("Finished in background");
    expect(other.messages[1].content).toBe("A complete answer.");
    expect(expanded.history[1].content).toBe("Finished in background");
  });

  it("stops during source flushing without starting the native run", async () => {
    const saving = deferred();
    mocks.flush.mockReturnValueOnce(saving.promise);
    const source = getSourceConversation(graphPath, "page", "Book");
    source.draft = "Question";
    const sending = sendAssistantQuestion(source);
    await vi.waitFor(() => expect(mocks.flush).toHaveBeenCalled());
    await stopAssistantConversation(source);
    saving.resolve();
    await sending;
    expect(mocks.chat).not.toHaveBeenCalled();
    expect(source.state.kind).toBe("cancelled");
  });

  it("retains partial answers and rejects late events after Stop and a new run", async () => {
    const pending = deferred();
    let events!: ResearchStreamHandlers;
    mocks.chat.mockImplementationOnce((_request, handlers) => { events = handlers; return pending.promise; });
    const source = getGlobalConversation(graphPath);
    source.draft = "First";
    const sending = sendAssistantQuestion(source);
    await vi.waitFor(() => expect(events).toBeDefined());
    events.onChunk("Partial");
    const id = source.requestId;
    await stopAssistantConversation(source);
    expect(mocks.cancel).toHaveBeenCalledWith(id);
    source.draft = "Second";
    await sendAssistantQuestion(source);
    events.onChunk(" late text");
    events.onSources?.([{ index: 1, page_id: "wrong", page_title: "Wrong", block_id: "wrong" }]);
    events.onError?.("late error");
    events.onDone();
    pending.resolve();
    await sending;
    expect(source.messages[1].content).toBe("Partial");
    expect(source.messages[1].sources).toBeUndefined();
    expect(source.messages[3].content).toBe("A complete answer.");
    expect(source.error).toBeNull();
  });

  it("reports graph changes and failed saves, preserving the question for retry", async () => {
    const source = getSourceConversation(graphPath, "page", "Book");
    source.draft = "Keep my question";
    mocks.graph.mockResolvedValueOnce({ path: "/wrong" });
    await sendAssistantQuestion(source);
    expect(source.error).toMatch(/graph changed/);
    expect(source.draft).toBe("Keep my question");
    expect(mocks.chat).not.toHaveBeenCalled();
    mocks.flush.mockRejectedValueOnce(new Error("Disk failure"));
    await sendAssistantQuestion(source);
    expect(source.error).toMatch(/Disk failure/);
    expect(source.history).toEqual([]);
  });

  it("rejects a graph change after the captured page has flushed", async () => {
    const source = getSourceConversation(graphPath, "page", "Book");
    source.draft = "Question";
    mocks.graph.mockResolvedValueOnce({ path: graphPath }).mockResolvedValueOnce({ path: "/other" });
    await sendAssistantQuestion(source);
    expect(source.error).toMatch(/graph changed while saving/);
    expect(mocks.chat).not.toHaveBeenCalled();
  });

  it("does not turn an invalid selection into a page-wide request", async () => {
    const source = getGlobalConversation(graphPath);
    source.context = { kind: "selection", pageId: "page", selection: { blockIds: [], text: "" } };
    source.draft = "Question";
    await sendAssistantQuestion(source);
    expect(source.error).toMatch(/Select a passage/);
    expect(source.draft).toBe("Question");
    expect(source.messages).toEqual([]);
    expect(mocks.chat).not.toHaveBeenCalled();
  });

  it("new conversations reset context and mode intentionally without touching sibling sources", async () => {
    const source = getSourceConversation(graphPath, "book", "Book", true);
    const global = getGlobalConversation(graphPath);
    global.draft = "My global draft";
    source.context = { kind: "none" };
    source.mode = "deep";
    source.draft = "Question";
    await sendAssistantQuestion(source);
    newAssistantConversation(source);
    expect(source).toMatchObject({ context: { kind: "book", pageId: "book" }, mode: "answer", messages: [], history: [], draft: "" });
    expect(global.draft).toBe("My global draft");
    global.context = { kind: "graph" };
    newAssistantConversation(global);
    expect(global.context).toEqual({ kind: "none" });
  });

  it("reports empty and incomplete streams rather than displaying a successful blank answer", async () => {
    const source = getGlobalConversation(graphPath);
    source.draft = "Question";
    mocks.chat.mockImplementationOnce(async (_request, handlers) => handlers.onDone());
    await sendAssistantQuestion(source);
    expect(source.error).toMatch(/no answer/);
    mocks.chat.mockResolvedValueOnce(undefined);
    await sendAssistantQuestion(source);
    expect(source.error).toMatch(/without a completion event/);
  });

  it("keeps unknown progress phases out of typed status without losing the answer", async () => {
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      const source = getGlobalConversation(graphPath);
      source.draft = "Question";
      mocks.chat.mockImplementationOnce(async (_request, handlers) => {
        handlers.onPhase?.("planning");
        expect(source.state.phase).toBe("planning");
        handlers.onPhase?.("__proto__");
        expect(source.state.phase).toBe("planning");
        handlers.onChunk("An answer.");
        handlers.onDone();
      });
      await sendAssistantQuestion(source);
      expect(warning).toHaveBeenCalledWith("[chat] Unknown progress phase:", "__proto__");
      expect(source.state.kind).toBe("done");
    } finally {
      warning.mockRestore();
    }
  });

  it("cancels all old runs when changing graph, preserving their visible results", async () => {
    const pending = deferred();
    let events!: ResearchStreamHandlers;
    mocks.chat.mockImplementationOnce((_request, handlers) => { events = handlers; return pending.promise; });
    const source = getGlobalConversation(graphPath);
    source.draft = "Question";
    const sending = sendAssistantQuestion(source);
    await vi.waitFor(() => expect(events).toBeDefined());
    events.onChunk("Partial");
    await stopAllAssistantConversations();
    expect(source.state.kind).toBe("cancelled");
    expect(source.messages[1].content).toBe("Partial");
    pending.resolve();
    await sending;
  });
});
