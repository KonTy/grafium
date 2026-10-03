import { get } from "svelte/store";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const saved = new Map<string, string>();

beforeEach(() => {
  saved.clear();
  vi.resetModules();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => { saved.set(key, value); },
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("recent pages limit", () => {
  it("accepts whole numbers from zero to the maximum and defaults invalid input", async () => {
    const { RECENT_PAGES_DEFAULT, RECENT_PAGES_MAX, normalizeRecentPagesLimit } = await import("./recentPagesLimit");
    expect(normalizeRecentPagesLimit(0)).toBe(0);
    expect(normalizeRecentPagesLimit("7")).toBe(7);
    expect(normalizeRecentPagesLimit(4.6)).toBe(5);
    expect(normalizeRecentPagesLimit(-3)).toBe(0);
    expect(normalizeRecentPagesLimit(500)).toBe(RECENT_PAGES_MAX);
    for (const invalid of ["", "  ", "many", Number.NaN, null, undefined]) {
      expect(normalizeRecentPagesLimit(invalid)).toBe(RECENT_PAGES_DEFAULT);
    }
  });

  it("starts from the remembered choice, updates subscribers immediately and saves it", async () => {
    saved.set("grafium.sidebar.recentPagesLimit", "4");
    const { recentPagesLimit, setRecentPagesLimit } = await import("./recentPagesLimit");
    expect(get(recentPagesLimit)).toBe(4);
    expect(setRecentPagesLimit("3")).toBe(3);
    expect(get(recentPagesLimit)).toBe(3);
    expect(saved.get("grafium.sidebar.recentPagesLimit")).toBe("3");
    expect(setRecentPagesLimit(0)).toBe(0);
    expect(get(recentPagesLimit)).toBe(0);
  });

  it("still applies the limit when it cannot be saved, and says so", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.stubGlobal("localStorage", {
      getItem: () => null,
      setItem: () => { throw new Error("quota"); },
    });
    const { recentPagesLimit, setRecentPagesLimit } = await import("./recentPagesLimit");
    expect(setRecentPagesLimit(12)).toBe(12);
    expect(get(recentPagesLimit)).toBe(12);
    expect(warn).toHaveBeenCalledWith("[sidebar] Could not remember the recent pages limit:", expect.any(Error));
  });
});
