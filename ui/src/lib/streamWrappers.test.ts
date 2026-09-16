import { describe, it, expect, vi, beforeEach } from "vitest";

// The stream wrappers must be exception-safe: a listen() rejection during setup
// has to route through onError (so Chat never gets stuck "active") and must not
// leak an already-acquired listener. We drive that by mocking the Tauri plumbing.
const invokeMock = vi.fn();
const listenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

import { researchDeep } from "./research";
import {
  aiAsk,
  aiAskStream,
  isResearchCancellation,
  RESEARCH_CANCELLED_MESSAGE,
} from "./knowledge";

type Handlers = {
  onChunk: (d: string) => void;
  onDone: () => void;
  onError: (m: string) => void;
};

function handlers(): Handlers {
  return { onChunk: vi.fn(), onDone: vi.fn(), onError: vi.fn() };
}

beforeEach(() => {
  invokeMock.mockReset();
  listenMock.mockReset();
});

describe("isResearchCancellation", () => {
  it("matches the canonical cancellation message and nothing else", () => {
    expect(isResearchCancellation(RESEARCH_CANCELLED_MESSAGE)).toBe(true);
    expect(isResearchCancellation(`Error: ${RESEARCH_CANCELLED_MESSAGE}`)).toBe(true);
    expect(isResearchCancellation("network unreachable")).toBe(false);
    expect(isResearchCancellation("")).toBe(false);
  });
});

// The two wrappers share the same setup/teardown shape, so exercise both.
const wrappers: Array<[string, (h: Handlers) => Promise<void>]> = [
  ["researchDeep", (h) => researchDeep("q", h)],
  ["aiAskStream", (h) => aiAskStream("q", h)],
];

describe("explicit chat scope", () => {
  beforeEach(() => {
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockResolvedValue(undefined);
  });

  it("defaults ordinary chat to Local graph even for web-worded questions", async () => {
    await aiAskStream("Search the web for test results", handlers());
    expect(invokeMock).toHaveBeenCalledWith("ai_ask_stream",
      expect.objectContaining({ scope: "local", history: [] }));
  });

  it("keeps page and block source IDs separate from the actual Ask question", async () => {
    const question = "What does this video say about training?";
    const history = [{ role: "user" as const, content: "Tell me about the video" }];
    for (const contextTarget of [{ pageId: "video-page" }, { pageId: "video-page", blockId: "transcript" }]) {
      await aiAsk(question, undefined, history, contextTarget);
      expect(invokeMock).toHaveBeenLastCalledWith("ai_ask", {
        question, graphId: undefined, history, contextTarget,
      });
    }
    await aiAsk(question, undefined, history);
    expect(invokeMock).toHaveBeenLastCalledWith("ai_ask", {
      question, graphId: undefined, history,
    });
  });

  it("passes Internet independently of the research toggle", async () => {
    const history = [{ role: "user" as const, content: "Tell me about test results" }];
    await aiAskStream("What is new?", handlers(), undefined, history, "internet");
    expect(invokeMock).toHaveBeenCalledWith("ai_ask_stream",
      expect.objectContaining({ scope: "internet", history }));
  });

  it("explicitly opts deep research into Internet", async () => {
    await researchDeep("test results", handlers());
    expect(invokeMock).toHaveBeenCalledWith("research_deep",
      expect.objectContaining({ scope: "internet" }));
  });

  it("rejects deep research in Local graph before attaching listeners", async () => {
    const h = handlers();
    await researchDeep("test results", h, undefined, undefined, undefined, "local");
    expect(h.onError).toHaveBeenCalledWith("Error: Select Internet scope to use Research.");
    expect(invokeMock).not.toHaveBeenCalled();
    expect(listenMock).not.toHaveBeenCalled();
  });
});

for (const [name, run] of wrappers) {
  describe(`${name} setup safety`, () => {
    it("reports a real invoke rejection through onError and removes both listeners", async () => {
      const u1 = vi.fn();
      const u2 = vi.fn();
      listenMock.mockResolvedValueOnce(u1).mockResolvedValueOnce(u2);
      invokeMock.mockRejectedValueOnce("boom");

      const h = handlers();
      await run(h);

      expect(h.onError).toHaveBeenCalledTimes(1);
      expect(h.onError).toHaveBeenCalledWith("boom");
      expect(u1).toHaveBeenCalledTimes(1);
      expect(u2).toHaveBeenCalledTimes(1);
    });

    it("swallows the canonical cancellation rejection as a normal end", async () => {
      const u1 = vi.fn();
      const u2 = vi.fn();
      listenMock.mockResolvedValueOnce(u1).mockResolvedValueOnce(u2);
      invokeMock.mockRejectedValueOnce(RESEARCH_CANCELLED_MESSAGE);

      const h = handlers();
      await run(h);

      // A user Stop is not a failure — onError must stay silent, listeners gone.
      expect(h.onError).not.toHaveBeenCalled();
      expect(u1).toHaveBeenCalledTimes(1);
      expect(u2).toHaveBeenCalledTimes(1);
    });

    it("routes a first-listener failure through onError without invoking the backend", async () => {
      listenMock.mockRejectedValueOnce(new Error("listen failed"));

      const h = handlers();
      await run(h);

      // Before the fix this rejected out of the wrapper, leaving Chat active forever.
      expect(h.onError).toHaveBeenCalledTimes(1);
      expect(invokeMock).not.toHaveBeenCalled();
    });

    it("removes the first listener when the second fails to attach (no leak)", async () => {
      const u1 = vi.fn();
      listenMock.mockResolvedValueOnce(u1).mockRejectedValueOnce(new Error("second failed"));

      const h = handlers();
      await run(h);

      expect(h.onError).toHaveBeenCalledTimes(1);
      expect(u1).toHaveBeenCalledTimes(1); // acquired listener cleaned up
      expect(invokeMock).not.toHaveBeenCalled();
    });
  });
}
