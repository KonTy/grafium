import { beforeEach, describe, expect, it, vi } from "vitest";
import { getResearchThread, newResearchConversation, researchTarget, researchWebMode, sendResearchQuestion, stopResearchThread } from "./researchThreads";
import type { ResearchStreamHandlers, ScopedResearchRequest } from "./research";
import { sourceReadingSelection } from "./readingSelection";

const mocks = vi.hoisted(() => ({
  graph: vi.fn(), flush: vi.fn(), research: vi.fn(), cancel: vi.fn(),
}));
vi.mock("./api", () => ({ getGraphInfo: mocks.graph }));
vi.mock("./editorPersistence", () => ({ flushPageEditors: mocks.flush }));
vi.mock("./research", () => ({ researchScoped: mocks.research, researchCancel: mocks.cancel }));

let graphNumber = 0;
let graphPath: string;
beforeEach(() => {
  vi.clearAllMocks();
  graphPath = `/synthetic/graph-${++graphNumber}`;
  mocks.graph.mockResolvedValue({ path: graphPath });
  mocks.flush.mockResolvedValue(undefined);
  mocks.cancel.mockResolvedValue(undefined);
  mocks.research.mockImplementation(async (_request, handlers) => { handlers.onChunk("An answer."); handlers.onDone(); });
});

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("scoped research threads", () => {
  it("persists each graph/page thread and restores its draft independently", () => {
    const a = getResearchThread(graphPath, "a", "A");
    const b = getResearchThread(graphPath, "b", "B");
    a.draft = "Keep this question";
    expect(getResearchThread(graphPath, "a", "Renamed A")).toBe(a);
    expect(a.pageTitle).toBe("Renamed A");
    expect(b.draft).toBe("");
    expect(getResearchThread("/other-graph", "a", "A").draft).toBe("");
    expect(a.internet).toBe(false);
    expect(a.research).toBe(false);
    expect(a.scope).toBe("page");
  });

  it("makes multi-step research impossible when Internet is off", () => {
    expect(researchWebMode(false, true)).toBe("off");
    expect(researchWebMode(false, false)).toBe("off");
    expect(researchWebMode(true, false)).toBe("search");
    expect(researchWebMode(true, true)).toBe("research");
  });

  it("creates explicit narrow targets without fallback widening", () => {
    const selection = sourceReadingSelection("a", "block", "before selected after", 7, 15)!;
    expect(researchTarget("a", "selection", null, null, selection)).toEqual({
      pageId: "a", scope: "selection", selection: { blockIds: ["block"], text: "selected" },
    });
    expect(researchTarget("a", "section", "cursor", "native-section", null)).toEqual({
      pageId: "a", scope: "section", blockId: "native-section",
    });
    expect(() => researchTarget("b", "selection", null, null, selection)).toThrow(/this page/);
    expect(() => researchTarget("a", "block", null, null, null)).toThrow(/Choose a block/);
    expect(() => researchTarget("a", "section", "cursor", null, null)).toThrow(/Choose a section/);
  });

  it("captures the question, target, history, and web mode before awaiting editor persistence", async () => {
    const flush = deferred();
    mocks.flush.mockReturnValue(flush.promise);
    const thread = getResearchThread(graphPath, "a", "Long book");
    thread.draft = "What does this mean?";
    const target = researchTarget("a", "selection", null, null, sourceReadingSelection("a", "block", "only this excerpt", 0, 17));
    const pending = sendResearchQuestion(thread, target);
    await vi.waitFor(() => expect(mocks.flush).toHaveBeenCalledWith("a"));
    expect(mocks.research).not.toHaveBeenCalled();
    target.selection!.text = "Something else";
    target.selection!.blockIds.push("wrong-block");
    thread.internet = true;
    thread.draft = "Next question";
    flush.resolve();
    await pending;
    expect(mocks.research).toHaveBeenCalledWith(expect.objectContaining({
      question: "What does this mean?", graphPath, webMode: "off", history: [],
      target: { pageId: "a", scope: "selection", selection: { text: "only this excerpt", blockIds: ["block"] } },
    }), expect.anything());
    expect(thread.draft).toBe("Next question");
  });

  it("updates the initiating thread while another source is visible and answering", async () => {
    const waiting = deferred();
    let callbacks!: ResearchStreamHandlers;
    mocks.research.mockImplementationOnce((_request, handlers) => { callbacks = handlers; return waiting.promise; });
    const a = getResearchThread(graphPath, "a", "A");
    a.draft = "Question A";
    const pending = sendResearchQuestion(a, { pageId: "a", scope: "page" });
    await vi.waitFor(() => expect(callbacks).toBeDefined());
    const b = getResearchThread(graphPath, "b", "B");
    b.draft = "Question B";
    await sendResearchQuestion(b, { pageId: "b", scope: "page" });
    callbacks.onChunk("Background A answer");
    callbacks.onDone();
    waiting.resolve();
    await pending;
    expect(a.messages[1].content).toBe("Background A answer");
    expect(b.messages[1].content).toBe("An answer.");
    expect(a.history[1].content).toBe("Background A answer");
    expect(getResearchThread(graphPath, "a", "A")).toBe(a);
  });

  it("sends successful prior turns on follow-ups and explicit New conversation clears them", async () => {
    const thread = getResearchThread(graphPath, "a", "A");
    thread.draft = "First";
    await sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    thread.draft = "Follow-up";
    await sendResearchQuestion(thread, { pageId: "a", scope: "block", blockId: "block" });
    const request = mocks.research.mock.calls[1][0] as ScopedResearchRequest;
    expect(request.question).toBe("Follow-up");
    expect(request.history).toEqual([{ role: "user", content: "First" }, { role: "assistant", content: "An answer." }]);
    newResearchConversation(thread);
    expect(thread.messages).toEqual([]);
    expect(thread.history).toEqual([]);
    expect(thread.state.kind).toBe("idle");
  });

  it("retains partial answers on Stop and ignores late chunks/errors/sources from a replaced run", async () => {
    const waiting = deferred();
    let callbacks!: ResearchStreamHandlers;
    mocks.research.mockImplementationOnce((_request, handlers) => { callbacks = handlers; return waiting.promise; });
    const thread = getResearchThread(graphPath, "a", "A");
    thread.draft = "First";
    const pending = sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    await vi.waitFor(() => expect(callbacks).toBeDefined());
    callbacks.onChunk("Partial");
    const requestId = thread.requestId;
    await stopResearchThread(thread);
    expect(mocks.cancel).toHaveBeenCalledWith(requestId);
    expect(thread.messages[1].content).toBe("Partial");
    expect(thread.state.kind).toBe("cancelled");
    thread.draft = "Next";
    await sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    callbacks.onChunk(" stale");
    callbacks.onError?.("stale error");
    callbacks.onSources?.([{ page_id: "wrong" } as never]);
    callbacks.onDone();
    waiting.resolve();
    await pending;
    expect(thread.messages[1].content).toBe("Partial");
    expect(thread.messages[1].sources).toBeUndefined();
    expect(thread.messages[3].content).toBe("An answer.");
    expect(thread.error).toBeNull();
  });

  it("stopping while saving prevents a native run from starting", async () => {
    const flush = deferred();
    mocks.flush.mockReturnValue(flush.promise);
    const thread = getResearchThread(graphPath, "a", "A");
    thread.draft = "Question";
    const pending = sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    await vi.waitFor(() => expect(mocks.flush).toHaveBeenCalled());
    await stopResearchThread(thread);
    flush.resolve();
    await pending;
    expect(mocks.research).not.toHaveBeenCalled();
    expect(thread.state.kind).toBe("cancelled");
  });

  it("reports graph switches and failed persistence visibly", async () => {
    const thread = getResearchThread(graphPath, "a", "A");
    thread.draft = "Question";
    mocks.graph.mockResolvedValueOnce({ path: "/wrong" });
    await sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    expect(thread.error).toMatch(/graph changed/);
    expect(mocks.research).not.toHaveBeenCalled();
    thread.draft = "Try again";
    mocks.flush.mockRejectedValueOnce(new Error("disk write failed"));
    await sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    expect(thread.error).toMatch(/disk write failed/);
    expect(thread.history).toEqual([]);
  });

  it("reports a failed cancellation without losing a partial answer", async () => {
    const waiting = deferred();
    let callbacks!: ResearchStreamHandlers;
    mocks.research.mockImplementationOnce((_request, handlers) => { callbacks = handlers; return waiting.promise; });
    mocks.cancel.mockRejectedValueOnce(new Error("cancel IPC unavailable"));
    const thread = getResearchThread(graphPath, "a", "A");
    thread.draft = "Question";
    const pending = sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    await vi.waitFor(() => expect(callbacks).toBeDefined());
    callbacks.onChunk("Partial");
    await stopResearchThread(thread);
    expect(thread.error).toMatch(/Could not confirm Stop.*cancel IPC unavailable/);
    expect(thread.messages[1].content).toBe("Partial");
    callbacks.onChunk(" ignored");
    waiting.resolve();
    await pending;
    expect(thread.messages[1].content).toBe("Partial");
  });

  it("treats an empty or unterminated stream as an explicit error", async () => {
    const thread = getResearchThread(graphPath, "a", "A");
    thread.draft = "Question";
    mocks.research.mockImplementationOnce(async (_request, handlers) => { handlers.onDone(); });
    await sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    expect(thread.error).toMatch(/no answer/);
    thread.draft = "Retry";
    mocks.research.mockResolvedValueOnce(undefined);
    await sendResearchQuestion(thread, { pageId: "a", scope: "page" });
    expect(thread.error).toMatch(/without a completion event/);
  });
});
