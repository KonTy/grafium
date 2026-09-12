import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  CHAT_SCOPE_PREF_KEY, loadChatScope, saveChatScope, loadChatPreferences, saveChatPreferences,
} from "./chatScope";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => {
  vi.restoreAllMocks();
  vi.mocked(invoke).mockReset();
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => { values.set(key, value); }),
  });
});

describe("native Chat preferences", () => {
  it("restores preferences without depending on incognito browser storage", async () => {
    vi.mocked(invoke).mockResolvedValue({ scope: "internet", research: true });
    expect(await loadChatPreferences()).toEqual({ scope: "internet", research: true });
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("get_chat_preferences");
  });

  it("migrates existing preferences when native preferences do not exist", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    localStorage.setItem("grafium.chat.research", "1");
    expect(await loadChatPreferences()).toEqual({ scope: "local", research: true });
    expect(invoke).toHaveBeenLastCalledWith("set_chat_preferences", {
      preferences: { scope: "local", research: true },
    });
  });

  it("saves scope and Research together", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await saveChatPreferences({ scope: "internet", research: false });
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("set_chat_preferences", {
      preferences: { scope: "internet", research: false },
    });
    expect(loadChatScope()).toBe("internet");
  });

  it("surfaces native read failures instead of overwriting preferences", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("unreadable"));
    await expect(loadChatPreferences()).rejects.toThrow("unreadable");
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("surfaces native write failures without updating the browser cache", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("disk full"));
    await expect(saveChatPreferences({ scope: "internet", research: true }))
      .rejects.toThrow("disk full");
    expect(localStorage.setItem).not.toHaveBeenCalled();
  });
});

afterEach(() => vi.unstubAllGlobals());

describe("chat scope preference", () => {
  it("defaults to Local graph even when an old Research preference is enabled", () => {
    localStorage.setItem("grafium.chat.research", "1");
    expect(loadChatScope()).toBe("local");
  });

  it("remembers each selection until it is changed", () => {
    saveChatScope("internet");
    expect(loadChatScope()).toBe("internet");
    expect(loadChatScope()).toBe("internet");
    saveChatScope("local");
    expect(loadChatScope()).toBe("local");
  });

  it("treats unknown saved values as Local graph", () => {
    localStorage.setItem(CHAT_SCOPE_PREF_KEY, "auto");
    expect(loadChatScope()).toBe("local");
  });

  it("keeps the safe default and reports unavailable storage", () => {
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.mocked(localStorage.getItem).mockImplementation(() => { throw new Error("blocked"); });
    expect(loadChatScope()).toBe("local");
    expect(warning).toHaveBeenCalledOnce();
  });

  it("reports persistence failures without blocking the current selection", () => {
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.mocked(localStorage.setItem).mockImplementation(() => { throw new Error("full"); });
    expect(() => saveChatScope("internet")).not.toThrow();
    expect(warning).toHaveBeenCalledOnce();
  });
});
