import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import {
  acquireChatSlot, cancelChatSlot, chatQueueDepth, deriveChatTitle, loadChatConcurrency,
  parseContext, parseMode, queueWaitLabel, resetChatConcurrency, resetChatQueue,
  serializeContext, shouldApplyChatTitle,
} from "./chatStore";

const invoked = invoke as unknown as ReturnType<typeof vi.fn>;

function serialProvider(slots = 1) {
  invoked.mockResolvedValue({ parallel: false, slots, provider: "local" });
}

function parallelProvider() {
  invoked.mockResolvedValue({ parallel: true, slots: null, provider: "cloud" });
}

/** `acquireChatSlot` awaits the capability check, so one microtask isn't
 *  enough to see a queue settle. */
function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  invoked.mockReset();
  resetChatConcurrency();
  resetChatQueue();
});

describe("chat queue", () => {
  it("lets a cloud provider run everything at once", async () => {
    parallelProvider();
    const positions: number[] = [];
    await acquireChatSlot("a", (p) => positions.push(p));
    await acquireChatSlot("b", (p) => positions.push(p));
    await acquireChatSlot("c", (p) => positions.push(p));
    expect(positions).toEqual([]);
    expect(chatQueueDepth()).toBe(0);
  });

  /**
   * The bug this exists to prevent: against the embedded model a second chat
   * used to block inside the Rust worker with no feedback at all, which looks
   * exactly like the app hanging.
   */
  it("makes a second chat wait when the model serves one at a time", async () => {
    serialProvider();
    const release = await acquireChatSlot("first", () => {});

    let secondStarted = false;
    const positions: number[] = [];
    const pending = acquireChatSlot("second", (p) => positions.push(p)).then((done) => {
      secondStarted = true;
      return done;
    });

    await flush();
    expect(secondStarted).toBe(false);
    expect(positions).toEqual([1]);

    release();
    const done = await pending;
    expect(secondStarted).toBe(true);
    expect(positions.at(-1)).toBe(0);
    done();
  });

  it("tells each waiting chat how many are ahead of it", async () => {
    serialProvider();
    const release = await acquireChatSlot("running", () => {});
    const second: number[] = [];
    const third: number[] = [];
    void acquireChatSlot("second", (p) => second.push(p));
    void acquireChatSlot("third", (p) => third.push(p));
    await flush();

    expect(second.at(-1)).toBe(1);
    expect(third.at(-1)).toBe(2);

    release();
    await flush();
    expect(third.at(-1)).toBe(1);
  });

  it("moves everyone up when a queued chat is stopped", async () => {
    serialProvider();
    const release = await acquireChatSlot("running", () => {});
    void acquireChatSlot("doomed", () => {});
    const last: number[] = [];
    void acquireChatSlot("last", (p) => last.push(p));
    await flush();
    expect(last.at(-1)).toBe(2);

    cancelChatSlot("doomed");
    await flush();
    expect(last.at(-1)).toBe(1);
    release();
  });

  it("frees the slot when a running chat is stopped", async () => {
    serialProvider();
    const release = await acquireChatSlot("running", () => {});
    let ran = false;
    const pending = acquireChatSlot("next", () => {}).then((done) => {
      ran = true;
      return done;
    });
    await flush();
    expect(ran).toBe(false);
    release();
    await pending;
    expect(ran).toBe(true);
  });

  it("honours more than one slot when the provider reports them", async () => {
    serialProvider(2);
    const first = await acquireChatSlot("a", () => {});
    let secondRan = false;
    await acquireChatSlot("b", () => {}).then(() => { secondRan = true; });
    expect(secondRan).toBe(true);

    let thirdRan = false;
    void acquireChatSlot("c", () => {}).then(() => { thirdRan = true; });
    await flush();
    expect(thirdRan).toBe(false);
    first();
  });

  /** A provider that can't be asked must not make the user wait on a guess. */
  it("runs in parallel when the capability check fails", async () => {
    invoked.mockRejectedValue(new Error("no provider"));
    const capability = await loadChatConcurrency();
    expect(capability.parallel).toBe(true);
    await acquireChatSlot("a", () => {});
    await acquireChatSlot("b", () => {});
    expect(chatQueueDepth()).toBe(0);
  });

  it("only asks the backend once", async () => {
    serialProvider();
    await Promise.all([loadChatConcurrency(), loadChatConcurrency(), loadChatConcurrency()]);
    expect(invoked).toHaveBeenCalledTimes(1);
  });
});

describe("queue wait label", () => {
  it("says nothing when the chat is running", () => {
    expect(queueWaitLabel(0, "local")).toBe("");
  });

  it("names the provider and the place in line", () => {
    expect(queueWaitLabel(1, "local")).toBe("Waiting for the local model — next in line");
    expect(queueWaitLabel(3, "local")).toBe("Waiting for the local model — 3 ahead");
  });

  it("still reads properly without a provider name", () => {
    expect(queueWaitLabel(1, "")).toBe("Waiting for the model — next in line");
  });
});

describe("conversation titles", () => {
  it("names a chat after its opening question", () => {
    expect(deriveChatTitle("  What can the VIVO X300 Ultra do? ")).toBe(
      "What can the VIVO X300 Ultra do?",
    );
  });

  it("collapses newlines so the switcher stays one line per chat", () => {
    expect(deriveChatTitle("first line\n\nsecond line")).toBe("first line second line");
  });

  it("clips a long question at a word boundary", () => {
    const title = deriveChatTitle(
      "Tell me everything about flashing a global operating system image onto a phone",
    );
    expect(title.length).toBeLessThanOrEqual(49);
    expect(title.endsWith("…")).toBe(true);
    expect(title).not.toContain("  ");
  });

  it("falls back rather than showing an empty row", () => {
    expect(deriveChatTitle("   ")).toBe("New chat");
  });
});

describe("stored context", () => {
  it("round-trips a page context", () => {
    const context = { kind: "page", pageId: "abc" } as const;
    expect(parseContext(serializeContext(context))).toEqual(context);
  });

  it("falls back to no notes when the stored context is unreadable", () => {
    expect(parseContext("{not json")).toEqual({ kind: "none" });
    expect(parseContext("[]")).toEqual({ kind: "none" });
  });

  it("keeps only modes the app knows", () => {
    expect(parseMode("deep")).toBe("deep");
    expect(parseMode("web")).toBe("web");
    expect(parseMode("something-else")).toBe("answer");
  });
});

describe("shouldApplyChatTitle", () => {
  const placeholder = "Can you flash a global OS image onto it?";
  const base = { placeholder, current: placeholder, titleIsCustom: false };

  it("replaces the placeholder with the name the model wrote", () => {
    expect(shouldApplyChatTitle("Flashing a global OS image", base)).toBe(true);
  });

  it("leaves a name the user typed alone", () => {
    expect(shouldApplyChatTitle("Flashing a global OS image", {
      ...base, current: "Phone stuff", titleIsCustom: true,
    })).toBe(false);
  });

  // The rename can land while the model is still thinking. Whatever is on
  // screen then is newer than what we are holding.
  it("leaves a name that changed while the model was thinking alone", () => {
    expect(shouldApplyChatTitle("Flashing a global OS image", {
      ...base, current: "Something else",
    })).toBe(false);
  });

  it("ignores an empty or blank suggestion", () => {
    expect(shouldApplyChatTitle("", base)).toBe(false);
    expect(shouldApplyChatTitle("   \n ", base)).toBe(false);
  });

  it("does not churn when the model suggests what is already there", () => {
    expect(shouldApplyChatTitle(`  ${placeholder}  `, base)).toBe(false);
  });
});
