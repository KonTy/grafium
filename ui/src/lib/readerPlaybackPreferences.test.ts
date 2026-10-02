import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
const toast = vi.hoisted(() => vi.fn());
vi.mock("./toast.svelte", () => ({ showToast: toast }));
import { applyReaderPlaybackRate, loadReaderPlaybackPreferences, mediaPlaybackRate, speechPlaybackRate, setMediaPlaybackRate, setSpeechPlaybackRate } from "./readerPlaybackPreferences";
const saved = new Map<string, string>();
beforeEach(() => {
  saved.clear(); toast.mockReset(); mediaPlaybackRate.set(1); speechPlaybackRate.set(1);
  vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => saved.set(key, value) });
});
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
describe("remembered media and speech speeds", () => {
  it("remembers distinct speeds across restarts without graph or model changes", () => {
    setMediaPlaybackRate(4); setSpeechPlaybackRate(1.5);
    mediaPlaybackRate.set(1); speechPlaybackRate.set(1);
    loadReaderPlaybackPreferences();
    expect(get(mediaPlaybackRate)).toBe(4); expect(get(speechPlaybackRate)).toBe(1.5);
    expect(saved.size).toBe(2);
  });
  it.each([NaN, Infinity, 0, -1, 0.49, 4.01])("rejects invalid speed %s before writing", value => {
    expect(() => setMediaPlaybackRate(value)).toThrow("between");
    expect(() => setSpeechPlaybackRate(value)).toThrow("between");
    expect(saved.size).toBe(0);
  });
  it("reports corrupt or unavailable storage without silently applying invalid rates", () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    saved.set("grafium.library.speechPlaybackRate", "999");
    loadReaderPlaybackPreferences();
    expect(get(speechPlaybackRate)).toBe(1);
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("could not be restored"), "error");
    vi.stubGlobal("localStorage", { setItem() { throw new Error("Denied"); } });
    setSpeechPlaybackRate(2);
    expect(get(speechPlaybackRate)).toBe(2);
    expect(toast).toHaveBeenLastCalledWith(expect.stringContaining("will not survive a restart"), "error");
  });
  it("changes actual media rate with pitch preserved without changing position or play state", () => {
    const element = document.createElement("audio");
    element.currentTime = 23.7;
    applyReaderPlaybackRate(element, 4);
    expect(element.playbackRate).toBe(4); expect(element.defaultPlaybackRate).toBe(4);
    expect(element.preservesPitch).toBe(true); expect(element.currentTime).toBe(23.7);
    expect(element.paused).toBe(true);
  });
  it("rejects clamped speeds and restores the old rate", () => {
    const element = document.createElement("audio");
    let rate = 1;
    Object.defineProperty(element, "playbackRate", { get: () => rate, set: value => { rate = Math.min(2, value); } });
    expect(() => applyReaderPlaybackRate(element, 4)).toThrow("did not accept");
    expect(element.playbackRate).toBe(1); expect(element.defaultPlaybackRate).toBe(1);
  });
});
