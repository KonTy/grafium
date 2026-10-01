import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { SvelteMap } from "svelte/reactivity";
import type { StudyItem } from "../lib/studies";
const api = vi.hoisted(() => ({ invoke: vi.fn(), open: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: api.open }));
import StudyMedia from "./StudyMedia.svelte";

let component: ReturnType<typeof mount> | undefined;
const fixture = (extra: Partial<StudyItem> = {}): StudyItem => ({
  id: "study", title: "Lesson", topic: "", kind: "audio", source: "assets/lesson.mp3",
  progress: { position: 42, total: 100, anchor: "", label: "" }, createdAt: "", updatedAt: "", ...extra,
});
const progress = vi.fn(); const playback = vi.fn(); const activity = vi.fn();
const wrapperOrigin = "http://127.0.0.1:32123";
const wrapperUrl = `${wrapperOrigin}/test-token/player#fixture`;
function render(item = fixture()) {
  component = mount(StudyMedia, { target: document.body, props: { graphPath: "/graph", item, onProgress: progress, onPlayback: playback, onActivity: activity } });
  flushSync();
}
afterEach(async () => { if (component) await unmount(component); component = undefined; document.body.replaceChildren(); vi.useRealTimers(); vi.restoreAllMocks(); });
beforeEach(() => {
  vi.resetAllMocks();
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
  api.invoke.mockImplementation(async command => command === "study_youtube_embed" ? wrapperUrl : "data:audio/mpeg;base64,AAAA");
  api.open.mockResolvedValue(undefined);
});

describe("study media", () => {
  it("restores native media before requesting playback and uses actual playback events", async () => {
    render();
    await vi.waitFor(() => expect(document.querySelector("audio")).not.toBeNull());
    expect(api.invoke).toHaveBeenCalledWith("read_asset_data_url", { path: "assets/lesson.mp3", graphPath: "/graph" });
    const audio = document.querySelector("audio")!;
    expect(audio.autoplay).toBe(false);
    expect(audio.currentTime).toBe(0);
    Object.defineProperty(audio, "duration", { value: 100, configurable: true });
    Object.defineProperty(audio, "readyState", { value: 1, configurable: true });
    audio.dispatchEvent(new Event("loadedmetadata"));
    expect(audio.currentTime).toBe(42);
    expect(audio.play).toHaveBeenCalledOnce();
    expect(progress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 42, total: 100 }));
    audio.dispatchEvent(new Event("playing")); expect(playback).toHaveBeenLastCalledWith(true);
    progress.mockClear();
    for (let repeat = 0; repeat < 10; repeat++) audio.dispatchEvent(new Event("timeupdate"));
    expect(progress).not.toHaveBeenCalled();
    for (const position of [43, 44, 45]) {
      audio.currentTime = position; audio.dispatchEvent(new Event("timeupdate"));
      expect(progress).toHaveBeenLastCalledWith(expect.objectContaining({ position }));
    }
    for (const event of ["waiting", "stalled", "ended"]) {
      audio.dispatchEvent(new Event(event));
      expect(playback).toHaveBeenLastCalledWith(false);
      audio.dispatchEvent(new Event("playing"));
    }
    audio.currentTime = 50; audio.dispatchEvent(new Event("pause"));
    expect(progress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 50 }));
    expect(playback).toHaveBeenLastCalledWith(false);
    audio.dispatchEvent(new Event("error")); flushSync();
    expect(document.body.textContent).toContain("could not be played");
  });
  it("keeps the restored position and explains blocked automatic playback", async () => {
    vi.mocked(HTMLMediaElement.prototype.play).mockRejectedValue(new DOMException("Blocked", "NotAllowedError"));
    render();
    await vi.waitFor(() => expect(document.querySelector("audio")).not.toBeNull());
    const audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "duration", { value: 100 });
    Object.defineProperty(audio, "readyState", { value: 1 });
    audio.dispatchEvent(new Event("loadedmetadata"));
    await vi.waitFor(() => expect(document.body.textContent).toContain("automatic playback is blocked"));
    expect(audio.currentTime).toBe(42);
    expect(playback).not.toHaveBeenCalledWith(true);
  });
  it("surfaces graph asset read failures", async () => {
    api.invoke.mockRejectedValue(new Error("Asset not found"));
    render();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Asset not found"));
    expect(document.querySelector("audio")).toBeNull();
  });
  it("surfaces resume failures with an explicit retry and user-triggered start", async () => {
    render();
    await vi.waitFor(() => expect(document.querySelector("audio")).not.toBeNull());
    const audio = document.querySelector("audio")!;
    Object.defineProperty(audio, "duration", { value: 100, configurable: true });
    Object.defineProperty(audio, "readyState", { value: 1, configurable: true });
    const seek = vi.fn(() => { throw new Error("Seek unavailable"); });
    Object.defineProperty(audio, "currentTime", { get: () => 0, set: seek, configurable: true });
    const play = vi.spyOn(audio, "play").mockResolvedValue();
    audio.dispatchEvent(new Event("loadedmetadata")); flushSync();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Could not restore your saved position");
    expect(progress).not.toHaveBeenCalled();
    document.querySelector<HTMLButtonElement>(".resume-actions button")!.click();
    expect(seek).toHaveBeenCalledTimes(2);
    document.querySelectorAll<HTMLButtonElement>(".resume-actions button")[1].click();
    await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
    expect(progress).toHaveBeenCalledWith(expect.objectContaining({ position: 0, total: 100 }));
    expect(document.querySelector(".resume-actions")).toBeNull();
  });
  it("does not reload playback when the parent updates progress, and drops stale graph reads", async () => {
    let finishRead: (value: string) => void = () => {};
    api.invoke.mockReturnValueOnce(new Promise<string>(resolve => { finishRead = resolve; }));
    const props = new SvelteMap<string, string | StudyItem>([["graph", "/first"], ["item", fixture()]]);
    component = mount(StudyMedia, { target: document.body, props: {
      get graphPath() { return props.get("graph") as string; },
      get item() { return props.get("item") as StudyItem; },
      onProgress: progress, onPlayback: playback,
    } });
    flushSync();
    props.set("graph", "/second"); flushSync();
    await vi.waitFor(() => expect(document.querySelector("audio")).not.toBeNull());
    const audio = document.querySelector("audio")!;
    const source = audio.src;
    finishRead("data:audio/mpeg;base64,OLD"); await Promise.resolve(); flushSync();
    expect(audio.src).toBe(source);
    const callCount = api.invoke.mock.calls.length;
    props.set("item", fixture({ progress: { position: 55, total: 100, anchor: "", label: "55%" } })); flushSync();
    expect(api.invoke).toHaveBeenCalledTimes(callCount);
    expect(document.querySelector("audio")).toBe(audio);
  });
  it("accepts player messages only from the exact isolated wrapper iframe and origin", async () => {
    render(fixture({ kind: "youtube", source: "https://youtu.be/dQw4w9WgXcQ" }));
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts allow-same-origin");
    expect(frame.src).toBe(wrapperUrl);
    expect(api.invoke).toHaveBeenCalledWith("study_youtube_embed", { videoId: "dQw4w9WgXcQ", start: 42 });
    expect(document.querySelector("script[src]")).toBeNull();
    const post = vi.spyOn(frame.contentWindow!, "postMessage");
    frame.dispatchEvent(new Event("load"));
    expect(post).toHaveBeenCalledWith(expect.stringContaining('"event":"listening"'), wrapperOrigin);
    const send = (origin: string, source: Window | null, info: unknown) => {
      window.dispatchEvent(new MessageEvent("message", { source, origin, data: JSON.stringify({ event: "infoDelivery", info }) })); flushSync();
    };
    send("https://evil.test", frame.contentWindow, { currentTime: 60, duration: 100, playerState: 1 });
    send(wrapperOrigin, window, { currentTime: 60, duration: 100, playerState: 1 });
    send("https://www.youtube-nocookie.com", frame.contentWindow, { currentTime: 60, duration: 100, playerState: 1 });
    expect(progress).not.toHaveBeenCalled(); expect(playback).not.toHaveBeenCalledWith(true);
    send(wrapperOrigin, frame.contentWindow, { currentTime: 0, duration: 100 });
    expect(progress).not.toHaveBeenCalled();
    send(wrapperOrigin, frame.contentWindow, { currentTime: 60, duration: 100, playerState: 1 });
    expect(progress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 60, total: 100 }));
    expect(playback).toHaveBeenLastCalledWith(true);
    progress.mockClear();
    for (let repeat = 0; repeat < 10; repeat++) {
      send(wrapperOrigin, frame.contentWindow, { currentTime: 60, duration: 100, playerState: 1 });
    }
    expect(progress).not.toHaveBeenCalled();
    for (const position of [61, 62, 63]) {
      send(wrapperOrigin, frame.contentWindow, { currentTime: position, duration: 100, playerState: 1 });
      expect(progress).toHaveBeenLastCalledWith(expect.objectContaining({ position }));
    }
    send(wrapperOrigin, frame.contentWindow, { playerState: 3 });
    expect(playback).toHaveBeenLastCalledWith(false);
  });
  it("provides blocked YouTube fallback without loading an external API script", () => {
    vi.useFakeTimers();
    render(fixture({ kind: "youtube", source: "https://youtu.be/dQw4w9WgXcQ" }));
    vi.advanceTimersByTime(15001); flushSync();
    expect(document.body.textContent).toContain("did not become available");
    expect(document.body.textContent).toContain("Open source in browser");
  });
  it("preserves the saved YouTube timestamp in the external fallback", async () => {
    render(fixture({ kind: "youtube", source: "https://youtu.be/dQw4w9WgXcQ" }));
    document.querySelector<HTMLButtonElement>(".external")!.click();
    await vi.waitFor(() => expect(api.open).toHaveBeenCalledWith("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s"));
  });
  it("recovers when saved YouTube progress exceeds the video's current duration", async () => {
    render(fixture({ kind: "youtube", source: "https://youtu.be/dQw4w9WgXcQ",
      progress: { position: 500, total: 1000, anchor: "", label: "" } }));
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const send = (info: unknown) => {
      window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin: wrapperOrigin,
        data: JSON.stringify({ event: "infoDelivery", info }) })); flushSync();
    };
    send({ duration: 100 });
    expect(document.body.textContent).toContain("beyond this video's current duration");
    send({ currentTime: 3, playerState: 1 });
    expect(progress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 3, total: 100 }));
    send({ currentTime: 4, playerState: 1 });
    expect(progress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 4, total: 100 }));
  });
  it.each(["https://127.0.0.1:32123/player", "http://localhost:32123/player", "http://example.com:32123/player", "http://user@127.0.0.1:32123/player"])("rejects an unexpected player endpoint %s", async endpoint => {
    api.invoke.mockResolvedValue(endpoint);
    render(fixture({ kind: "youtube", source: "https://youtu.be/dQw4w9WgXcQ" }));
    await vi.waitFor(() => expect(document.body.textContent).toContain("invalid local address"));
    expect(document.querySelector("iframe")).toBeNull();
  });
  it("ignores late player URLs after a graph switch", async () => {
    let finish: (value: string) => void = () => {};
    api.invoke.mockReturnValueOnce(new Promise<string>(resolve => { finish = resolve; }));
    const props = new SvelteMap([["graph", "/first"]]);
    component = mount(StudyMedia, { target: document.body, props: {
      get graphPath() { return props.get("graph")!; },
      item: fixture({ kind: "youtube", source: "https://youtu.be/dQw4w9WgXcQ" }),
      onProgress: progress, onPlayback: playback,
    } });
    flushSync(); props.set("graph", "/second"); flushSync();
    await vi.waitFor(() => expect(document.querySelector("iframe")?.src).toBe(wrapperUrl));
    finish("http://127.0.0.1:54321/stale/player"); await Promise.resolve(); flushSync();
    expect(document.querySelector("iframe")?.src).toBe(wrapperUrl);
  });
  it("opens websites externally and records only manual checkpoints", async () => {
    render(fixture({ kind: "website", source: "https://example.com/course" }));
    expect(document.querySelector("iframe")).toBeNull();
    expect(api.invoke).not.toHaveBeenCalled();
    document.querySelector<HTMLButtonElement>(".primary")!.click();
    await vi.waitFor(() => expect(api.open).toHaveBeenCalledWith("https://example.com/course"));
    const input = document.querySelector<HTMLInputElement>('input:not([type="number"])')!;
    input.value = "Chapter 4"; input.dispatchEvent(new Event("input", { bubbles: true }));
    const percentage = document.querySelector<HTMLInputElement>('input[type="number"]')!;
    percentage.value = "75"; percentage.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); flushSync();
    expect(progress).toHaveBeenCalledWith({ position: 75, total: 100, label: "Chapter 4", anchor: "" });
    expect(playback).not.toHaveBeenCalledWith(true); expect(activity).not.toHaveBeenCalled();
  });
  it("rejects unsafe persisted URLs before rendering or invoking any native operation", () => {
    render(fixture({ kind: "website", source: "javascript:alert(1)" }));
    expect(document.querySelector<HTMLButtonElement>(".primary")?.disabled).toBe(true);
    expect(api.invoke).not.toHaveBeenCalled(); expect(api.open).not.toHaveBeenCalled();
    expect(document.querySelector('[role="alert"]')).not.toBeNull();
  });
  it("rejects checkpoints exceeding the backend byte limit without emitting unsavable progress", () => {
    render(fixture({ kind: "website", source: "https://example.com/course" }));
    const input = document.querySelector<HTMLInputElement>('input:not([type="number"])')!;
    input.value = "\u00e9".repeat(513); input.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); flushSync();
    expect(progress).not.toHaveBeenCalled();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("1,024 UTF-8 bytes");
  });
});
