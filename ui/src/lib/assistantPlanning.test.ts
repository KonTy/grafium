import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ResearchStreamHandlers } from "./research";
const mocks = vi.hoisted(() => ({ chat: vi.fn(), cancel: vi.fn(), toast: vi.fn() }));
vi.mock("./assistant", async importOriginal => ({
  ...await importOriginal<typeof import("./assistant")>(), assistantChat: mocks.chat, assistantCancel: mocks.cancel,
}));
vi.mock("./toast.svelte", () => ({ showToast: mocks.toast }));
import { collectAssistantPlan } from "./assistantPlanning";

beforeEach(() => { vi.clearAllMocks(); mocks.cancel.mockResolvedValue(undefined); });
describe("cancellable edit planning", () => {
  it("uses the chosen scope without web requests or implicit history", async () => {
    mocks.chat.mockImplementation(async (_request, handlers: ResearchStreamHandlers) => {
      handlers.onChunk('{"actions":'); handlers.onChunk("[]}"); handlers.onDone();
    });
    expect(await collectAssistantPlan("/graph", "Plan", { kind: "none" }, new AbortController().signal)).toBe('{"actions":[]}');
    expect(mocks.chat).toHaveBeenCalledWith(expect.objectContaining({
      graphPath: "/graph", mode: "answer", context: { kind: "none" }, history: [],
    }), expect.any(Object));
  });
  it("cancels the exact native request and refuses its late answer", async () => {
    const controller = new AbortController();
    let finish!: () => void;
    mocks.chat.mockImplementation((_request, handlers: ResearchStreamHandlers) => new Promise<void>(resolve => {
      finish = () => { handlers.onChunk('{"actions":[]}'); resolve(); };
    }));
    const result = collectAssistantPlan("/graph", "Plan", { kind: "none" }, controller.signal);
    controller.abort();
    expect(mocks.cancel).toHaveBeenCalledWith(mocks.chat.mock.calls[0][0].requestId);
    expect(mocks.chat.mock.calls[0][1].shouldContinue()).toBe(false);
    finish();
    await expect(result).rejects.toMatchObject({ name: "AbortError" });
  });
  it("never starts an already cancelled request", async () => {
    const controller = new AbortController(); controller.abort();
    await expect(collectAssistantPlan("/graph", "Plan", { kind: "none" }, controller.signal)).rejects.toMatchObject({ name: "AbortError" });
    expect(mocks.chat).not.toHaveBeenCalled();
  });
  it("surfaces provider errors instead of accepting a partial plan", async () => {
    mocks.chat.mockImplementation(async (_request, handlers: ResearchStreamHandlers) => {
      handlers.onChunk('{"actions":[]}'); handlers.onError?.("Provider unavailable");
    });
    await expect(collectAssistantPlan("/graph", "Plan", { kind: "none" }, new AbortController().signal)).rejects.toThrow("Provider unavailable");
  });
  it("removes the cancellation listener after completion", async () => {
    const controller = new AbortController();
    mocks.chat.mockImplementation(async (_request, handlers: ResearchStreamHandlers) => handlers.onChunk('{"actions":[]}'));
    await collectAssistantPlan("/graph", "Plan", { kind: "none" }, controller.signal);
    controller.abort();
    expect(mocks.cancel).not.toHaveBeenCalled();
  });
});
