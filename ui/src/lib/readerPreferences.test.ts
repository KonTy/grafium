import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
const toast = vi.hoisted(() => vi.fn());
vi.mock("./toast.svelte", () => ({ showToast: toast }));
import { loadReaderTextSizePreference, readerTextSize, readerFlow, setReaderTextSize } from "./readerPreferences";

const saved = new Map<string, string>();
beforeEach(() => {
  saved.clear(); toast.mockReset(); readerTextSize.set(100); readerFlow.set("scrolled");
  vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => saved.set(key, value) });
});
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
describe("remembered book text size", () => {
  it("restores text size across reader mounts/restarts without changing layout", () => {
    setReaderTextSize(150); expect(saved.get("grafium.reader.textSize")).toBe("150");
    readerTextSize.set(100); loadReaderTextSizePreference();
    expect(get(readerTextSize)).toBe(150); expect(get(readerFlow)).toBe("scrolled");
  });
  it("keeps the current setting and reports corrupt saved values", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    saved.set("grafium.reader.textSize", "100000");
    readerTextSize.set(125); loadReaderTextSizePreference();
    expect(get(readerTextSize)).toBe(125); expect(warn).toHaveBeenCalledOnce();
    expect(() => setReaderTextSize(0)).toThrow("Unsupported");
  });
  it("reports persistence failures rather than claiming restart durability", () => {
    vi.stubGlobal("localStorage", { setItem() { throw new Error("Disk denied"); } });
    setReaderTextSize(200);
    expect(get(readerTextSize)).toBe(200);
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("will not survive a restart"), "error");
  });
});
