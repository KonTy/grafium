import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import LibrarySourcePlayer from "./LibrarySourcePlayer.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));
let component: ReturnType<typeof mount> | undefined;
const button = (text: string) => [...document.querySelectorAll("button")].find(element => element.textContent?.trim() === text)!;
beforeEach(() => {
  vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(async function (this: HTMLMediaElement) { this.dispatchEvent(new Event("playing")); });
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(function (this: HTMLMediaElement) { this.dispatchEvent(new Event("pause")); });
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.restoreAllMocks();
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
});
