import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import LibrarySourcePlayer from "./LibrarySourcePlayer.svelte";
import { get } from "svelte/store";
import { mediaPlaybackRate, speechPlaybackRate, setMediaPlaybackRate, loadReaderPlaybackPreferences } from "../lib/readerPlaybackPreferences";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => "http://127.0.0.1:5199/synthetic-youtube") }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));
let component: ReturnType<typeof mount> | undefined;
const button = (text: string) => [...document.querySelectorAll("button")].find(element => element.textContent?.trim() === text)!;
beforeEach(() => {
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => saved.set(key, value),
  });
  mediaPlaybackRate.set(1);
  speechPlaybackRate.set(1);
  vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(async function (this: HTMLMediaElement) { this.dispatchEvent(new Event("playing")); });
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(function (this: HTMLMediaElement) { this.dispatchEvent(new Event("pause")); });
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});
describe("visible foreground media controls", () => {
  it.each(["audio", "video"] as const)("controls %s with no autoplay, keyboard seeking, and stop on unmount", async kind => {
    const onPlayback = vi.fn(); const onProgress = vi.fn();
    component = mount(LibrarySourcePlayer, { target: document.body, props: {
      item: { id: kind, kind, title: "Synthetic media", source: `https://example.test/source.${kind === "audio" ? "mp3" : "mp4"}`,
        progress: { position: 10, total: 0, anchor: "", label: "" } },
      autoplay: false, onPlayback, onProgress,
    } });
    flushSync();
    const media = document.querySelector(kind)!;
    Object.defineProperties(media, {
      readyState: { value: 1 }, duration: { configurable: true, value: 100 },
      seekable: { configurable: true, value: { length: 1, start: () => 0, end: () => 100 } },
    });
    media.dispatchEvent(new Event("loadedmetadata")); flushSync();
    expect(media.play).not.toHaveBeenCalled();
    expect(media.currentTime).toBe(10);
    expect(button("Stop").disabled).toBe(false);
    const timeline = document.querySelector<HTMLInputElement>('input[type="range"]')!;
    expect(timeline.disabled).toBe(false);
    button("Resume").click(); flushSync();
    expect(onPlayback).toHaveBeenLastCalledWith(true);
    expect(button("Pause").disabled).toBe(false);
    button("Pause").click(); flushSync();
    expect(onPlayback).toHaveBeenLastCalledWith(false);
    timeline.value = "60"; timeline.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    expect(media.currentTime).toBe(60);
    expect(onProgress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 60, total: 100 }), false);
    expect(media.play).toHaveBeenCalledTimes(1);
    button("Resume").click(); flushSync();
    button("Stop").click(); flushSync();
    expect(document.querySelector('[role="status"]')?.textContent).toContain("Stopped");
    expect(media.currentTime).toBe(60);
    expect(onPlayback).toHaveBeenLastCalledWith(false);
    Object.defineProperty(media, "duration", { value: Infinity });
    media.dispatchEvent(new Event("durationchange")); flushSync();
    expect(timeline.disabled).toBe(true);
    expect(document.body.textContent).toContain("Duration unknown; seeking unavailable.");
    Object.defineProperty(media, "duration", { value: 100 });
    Object.defineProperty(media, "seekable", { value: { length: 0 } });
    media.dispatchEvent(new Event("durationchange")); flushSync();
    expect(timeline.disabled).toBe(true);
    expect(document.body.textContent).toContain("Seeking unavailable for this source.");
    await unmount(component!); component = undefined;
    expect(media.pause).toHaveBeenCalled();
    expect(media.getAttribute("src")).toBeNull();
  });
  it("Stop during loading prevents deferred autoplay", async () => {
    component = mount(LibrarySourcePlayer, { target: document.body, props: {
      item: { id: "audio", kind: "audio", title: "Pending audio", source: "https://example.test/audio.mp3",
        progress: { position: 0, total: 0, anchor: "", label: "" } },
      autoplay: true, onPlayback: vi.fn(), onProgress: vi.fn(),
    } });
    flushSync(); button("Stop").click();
    const media = document.querySelector("audio")!;
    Object.defineProperties(media, { readyState: { value: 1 }, duration: { value: 10 } });
    media.dispatchEvent(new Event("loadedmetadata")); flushSync();
    expect(media.play).not.toHaveBeenCalled();
    expect(document.querySelector('[role="status"]')?.textContent).toContain("Stopped");
  });
  it.each(["audio", "video"] as const)("applies and remembers 4× %s speed without playing or moving the saved position", async kind => {
    const props = {
      item: { id: kind, kind, title: "Synthetic speed", source: `https://example.test/source.${kind === "audio" ? "mp3" : "mp4"}`,
        progress: { position: 12, total: 100, anchor: "", label: "" } },
      autoplay: false, onPlayback: vi.fn(), onProgress: vi.fn(),
    };
    component = mount(LibrarySourcePlayer, { target: document.body, props });
    flushSync();
    let media = document.querySelector(kind)!;
    Object.defineProperties(media, { readyState: { value: 1 }, duration: { value: 100 } });
    media.dispatchEvent(new Event("loadedmetadata")); flushSync();
    const select = document.querySelector("select")!;
    expect(select.closest("label")?.textContent).toContain("Speed");
    expect([...select.options].map(option => option.value)).toEqual(["0.5", "0.75", "1", "1.25", "1.5", "1.75", "2", "2.5", "3", "3.5", "4"]);
    select.value = "4"; select.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    expect(media.playbackRate).toBe(4);
    expect(media.preservesPitch).toBe(true);
    expect(media.currentTime).toBe(12);
    expect(media.play).not.toHaveBeenCalled();
    expect(get(mediaPlaybackRate)).toBe(4);
    expect(get(speechPlaybackRate)).toBe(1);
    await unmount(component!); component = undefined;
    expect(media.pause).toHaveBeenCalled();
    mediaPlaybackRate.set(1);
    loadReaderPlaybackPreferences();
    expect(get(mediaPlaybackRate)).toBe(4);
    component = mount(LibrarySourcePlayer, { target: document.body, props: {
      ...props, item: { ...props.item, id: "next-source", source: props.item.source + "?next" },
    } });
    flushSync();
    media = document.querySelector(kind)!;
    expect(media.playbackRate).toBe(4);
    expect(document.querySelector("select")!.value).toBe("4");
    setMediaPlaybackRate(2.5); flushSync();
    expect(media.playbackRate).toBe(2.5);
    expect(document.querySelector("select")!.value).toBe("2.5");
    expect(media.play).not.toHaveBeenCalled();
  });
  it.each(["throw", "clamp"] as const)("reports a %s on unsupported speed without claiming 4× or forgetting the position", rejection => {
    component = mount(LibrarySourcePlayer, { target: document.body, props: {
      item: { id: "audio", kind: "audio", title: "Limited media", source: "https://example.test/audio.mp3",
        progress: { position: 12, total: 100, anchor: "", label: "" } },
      autoplay: false, onPlayback: vi.fn(), onProgress: vi.fn(),
    } });
    flushSync();
    const media = document.querySelector("audio")!;
    media.currentTime = 12;
    let actualRate = 1;
    Object.defineProperty(media, "playbackRate", {
      get: () => actualRate,
      set: rate => {
        if (rate > 2 && rejection === "throw") throw new DOMException("Synthetic unsupported rate", "NotSupportedError");
        actualRate = Math.min(2, rate);
      },
    });
    const select = document.querySelector("select")!;
    select.value = "4"; select.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Could not change playback speed");
    expect(select.value).toBe(String(media.playbackRate));
    expect(select.value).not.toBe("4");
    expect(get(mediaPlaybackRate)).toBe(1);
    expect(media.currentTime).toBe(12);
    media.dispatchEvent(new Event("playing")); flushSync();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Could not change playback speed");
    select.value = "1.5"; select.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    expect(media.playbackRate).toBe(1.5);
    expect(document.querySelector('[role="alert"]')).toBeNull();
  });
  it("delegates YouTube speed to the embedded player without claiming the saved media speed", () => {
    setMediaPlaybackRate(4);
    component = mount(LibrarySourcePlayer, { target: document.body, props: {
      item: { id: "youtube", kind: "youtube", title: "Provider controls", source: "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
        progress: { position: 0, total: 0, anchor: "", label: "" } },
      autoplay: false, onPlayback: vi.fn(), onProgress: vi.fn(),
    } });
    flushSync();
    const select = document.querySelector("select")!;
    expect(select.disabled).toBe(true);
    expect(select.selectedOptions[0].textContent).toBe("YouTube controls");
    expect(document.getElementById(select.getAttribute("aria-describedby")!)?.textContent)
      .toContain("Use YouTube's own speed controls; available speeds are set by YouTube.");
    expect(get(mediaPlaybackRate)).toBe(4);
  });
});
