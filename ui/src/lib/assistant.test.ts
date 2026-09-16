import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  assistantChat, assistantContextInfo, assistantCancel, copyAssistantContext,
  type AssistantRequest,
} from "./assistant";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

const request: AssistantRequest = {
  graphPath: "/synthetic/graph", requestId: "assistant-1", question: "What supports this claim?",
  context: { kind: "selection", pageId: "page", selection: { blockIds: ["b1"], text: "An exact claim." } },
  mode: "web", history: [],
};

beforeEach(() => {
  vi.resetAllMocks();
  mocks.invoke.mockResolvedValue(undefined);
  mocks.listen.mockResolvedValue(vi.fn());
});

describe("shared assistant IPC", () => {
  it("sends explicit context and mode, not source-concatenated questions or provider browsing flags", async () => {
    await assistantChat(request, { onChunk: vi.fn(), onDone: vi.fn() });
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_chat", request);
  });

  it.each(["answer", "web", "deep"] as const)("supports %s with an explicit no-notes context", async (mode) => {
    const input = { ...request, context: { kind: "none" as const }, mode };
    await assistantChat(input, { onChunk: vi.fn(), onDone: vi.fn() });
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_chat", input);
  });

  it("retrieves context capabilities and reuses the native cancellation registry", async () => {
    await assistantContextInfo("/graph", "book", "passage");
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_context_info", {
      graphPath: "/graph", pageId: "book", blockId: "passage",
    });
    await assistantCancel("operation");
    expect(mocks.invoke).toHaveBeenLastCalledWith("research_cancel", { requestId: "operation" });
  });

  it("routes only this request's events and releases both subscriptions", async () => {
    const callbacks = new Map<string, (event: unknown) => void>();
    const unlisten = vi.fn();
    mocks.listen.mockImplementation(async (name, callback) => { callbacks.set(name, callback); return unlisten; });
    mocks.invoke.mockImplementationOnce(async () => {
      callbacks.get("ai://chat_stream")!({ payload: { request_id: "other", delta: "Wrong" } });
      callbacks.get("ai://chat_sources")!({ payload: {
        request_id: request.requestId, sources: [], web_sources: [{ number: 1, title: "Evidence", url: "https://example.org" }],
      } });
      callbacks.get("ai://chat_stream")!({ payload: { request_id: request.requestId, delta: "Cited answer", done: true } });
    });
    const handlers = { onChunk: vi.fn(), onDone: vi.fn(), onWebSources: vi.fn() };
    await assistantChat(request, handlers);
    expect(handlers.onChunk).toHaveBeenCalledOnce();
    expect(handlers.onChunk).toHaveBeenCalledWith("Cited answer");
    expect(handlers.onDone).toHaveBeenCalledOnce();
    expect(handlers.onWebSources).toHaveBeenCalledWith([{ number: 1, title: "Evidence", url: "https://example.org" }]);
    expect(unlisten).toHaveBeenCalledTimes(2);
  });

  it("does not start model or web work after Stop during listener setup", async () => {
    let active = true;
    const unlisten = vi.fn();
    mocks.listen.mockImplementationOnce(async () => { active = false; return unlisten; });
    await assistantChat(request, { onChunk: vi.fn(), onDone: vi.fn(), shouldContinue: () => active });
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("surfaces a listener failure and releases acquired listeners", async () => {
    const unlisten = vi.fn();
    mocks.listen.mockResolvedValueOnce(unlisten).mockRejectedValueOnce("Source subscription failed");
    const onError = vi.fn();
    await assistantChat(request, { onChunk: vi.fn(), onDone: vi.fn(), onError });
    expect(onError).toHaveBeenCalledWith("Source subscription failed");
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("validates narrow contexts instead of falling back to the page", () => {
    expect(() => copyAssistantContext({ kind: "page", pageId: "" })).toThrow(/source page/);
    expect(() => copyAssistantContext({ kind: "section", pageId: "page", blockId: "" })).toThrow(/block or section/);
    expect(() => copyAssistantContext({ kind: "selection", pageId: "page", selection: { blockIds: ["a", "a"], text: "Text" } }))
      .toThrow(/Select a passage/);
    expect(copyAssistantContext({ kind: "none" })).toEqual({ kind: "none" });
  });
});
