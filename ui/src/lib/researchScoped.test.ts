import { beforeEach, describe, expect, it, vi } from "vitest";
import { researchScoped, researchScopeInfo, type ScopedResearchRequest } from "./research";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

const request: ScopedResearchRequest = {
  question: "Explain this passage", requestId: "scoped-1", graphPath: "/synthetic/graph",
  target: { pageId: "page-1", scope: "selection", selection: { blockIds: ["b1"], text: "Exact source" } },
  history: [{ role: "user", content: "Previous question" }], webMode: "off",
};

beforeEach(() => { vi.resetAllMocks(); mocks.invoke.mockResolvedValue(undefined); mocks.listen.mockResolvedValue(vi.fn()); });

describe("scoped research IPC contract", () => {
  it("passes scope info and scoped requests using camelCase without rewriting question text", async () => {
    const info = { pageId: "page-1", pageTitle: "Book", isJournal: false, isBook: true, blockCount: 10, section: { title: "Chapter", blockId: "heading" } };
    mocks.invoke.mockResolvedValueOnce(info);
    expect(await researchScopeInfo("/synthetic/graph", "page-1", "b1")).toEqual(info);
    expect(mocks.invoke).toHaveBeenCalledWith("research_scope_info", { graphPath: "/synthetic/graph", pageId: "page-1", blockId: "b1" });
    await researchScoped(request, { onChunk: vi.fn(), onDone: vi.fn() });
    expect(mocks.invoke).toHaveBeenLastCalledWith("research_scoped", request);
  });

  it("filters shared stream/source events by request ID and releases both listeners", async () => {
    const callbacks = new Map<string, (event: unknown) => void>();
    const unlisten = vi.fn();
    mocks.listen.mockImplementation(async (name, callback) => { callbacks.set(name, callback); return unlisten; });
    mocks.invoke.mockImplementationOnce(async () => {
      callbacks.get("ai://chat_stream")!({ payload: { request_id: "other", delta: "Wrong" } });
      callbacks.get("ai://chat_stream")!({ payload: { request_id: "scoped-1", phase: "retrieving", delta: "Answer", done: true } });
      callbacks.get("ai://chat_sources")!({ payload: { request_id: "scoped-1", sources: [{ page_id: "page-1" }] } });
    });
    const handlers = { onChunk: vi.fn(), onDone: vi.fn(), onSources: vi.fn(), onPhase: vi.fn() };
    await researchScoped(request, handlers);
    expect(handlers.onChunk).toHaveBeenCalledTimes(1);
    expect(handlers.onChunk).toHaveBeenCalledWith("Answer");
    expect(handlers.onDone).toHaveBeenCalledTimes(1);
    expect(handlers.onSources).toHaveBeenCalledWith([{ page_id: "page-1" }]);
    expect(unlisten).toHaveBeenCalledTimes(2);
  });

  it("reports setup errors and cleans up the first listener when the second fails", async () => {
    const unlisten = vi.fn();
    mocks.listen.mockResolvedValueOnce(unlisten).mockRejectedValueOnce("source listener failed");
    const onError = vi.fn();
    await researchScoped(request, { onChunk: vi.fn(), onDone: vi.fn(), onError });
    expect(onError).toHaveBeenCalledWith("source listener failed");
    expect(unlisten).toHaveBeenCalledOnce();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("does not start network/model work if stopped while listeners are attaching", async () => {
    let active = true;
    const unlisten = vi.fn();
    mocks.listen.mockImplementationOnce(async () => { active = false; return unlisten; });
    await researchScoped(request, { onChunk: vi.fn(), onDone: vi.fn(), shouldContinue: () => active });
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});
