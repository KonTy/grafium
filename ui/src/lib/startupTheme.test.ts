import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { applyTheme, themes } from "./themes";

beforeEach(() => {
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    get length() { return values.size; },
    key: (index: number) => [...values.keys()][index] ?? null,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
    clear: () => values.clear(),
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.documentElement.removeAttribute("style");
  document.documentElement.className = "";
});

describe("startup theme cache", () => {
  it("remembers the last applied colors without waiting for native settings next launch", () => {
    for (const theme of themes) {
      applyTheme(theme.colors);
      expect(JSON.parse(localStorage.getItem("grafium.startupTheme")!)).toEqual({
        background: theme.colors.bgPrimary,
        foreground: theme.colors.textPrimary,
        colorScheme: theme.colors.isLight ? "light" : "dark",
      });
    }
  });

  it("still applies the theme when storage is unavailable and reports the cache failure", () => {
    const error = new DOMException("Storage unavailable", "SecurityError");
    vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw error; });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const theme = themes.find((theme) => theme.id === "oled")!;
    applyTheme(theme.colors);
    expect(document.documentElement.style.getPropertyValue("--bg-primary")).toBe("#000000");
    expect(warn).toHaveBeenCalledWith("[startup] Could not cache the startup theme:", error);
  });
});
