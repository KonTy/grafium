import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { readFileSync } from "node:fs";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import Harness from "./PrivateReaderNavigation.test.svelte";
import { privateLibrary, type ReaderBook } from "../lib/privateReader";
import { playPrivateAudio, privatePlayback, stopPrivatePlayback } from "../lib/privateReaderPlayback";
import { get } from "svelte/store";

class FakeAudio extends EventTarget {
  src = ""; currentTime = 0; duration = 1000; preload = "";
  pause = vi.fn(); play = vi.fn(async () => {});
  load() { if (this.src) queueMicrotask(() => this.dispatchEvent(new Event("loadedmetadata"))); }
  removeAttribute() { this.src = ""; }
}
const book: ReaderBook = { id: "global-audio", title: "App-private audiobook", kind: "audio", available: true, tracks: [
  { id: "chapter", title: "Chapter", relativePath: "1.mp3" },
], position: { trackId: "chapter", offsetMs: 12000 }, bookmarks: [] };
let audio: FakeAudio;
let component: ReturnType<typeof mount> | undefined;
const button = (name: string) => [...document.querySelectorAll("button")].find(element => element.textContent?.trim() === name)!;
beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async command => command === "reader_media_url" ? "http://127.0.0.1:1234/capability"
    : command === "reader_snapshot" ? { libraryPath: "/local", books: [book] } : undefined);
  privateLibrary.set({ libraryPath: "/local", books: [book] });
  vi.stubGlobal("Audio", class extends FakeAudio { constructor() { super(); audio = this; } });
});
afterEach(async () => {
  await stopPrivatePlayback();
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.unstubAllGlobals();
});
describe("actual global reader controls", () => {
  it("keeps one player and working controls while the page and graph-owned content are replaced", async () => {
    component = mount(Harness, { target: document.body });
    await playPrivateAudio(book);
    flushSync();
    const bar = document.querySelector('[aria-label="Private reader playback"]');
    audio.currentTime = 42.25;
    button("Go to Journal").click(); flushSync();
    button("Switch graph").click(); flushSync();
    expect(document.querySelector("main")?.getAttribute("data-graph")).toBe("Other graph");
    expect(document.querySelector("h1")?.textContent).toBe("Journal");
    expect(document.querySelector('[aria-label="Private reader playback"]')).toBe(bar);
    expect(audio.currentTime).toBe(42.25);
    expect(audio.pause).not.toHaveBeenCalled();
    button("Bookmark").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Bookmark saved on this device."));
    expect(invoke).toHaveBeenCalledWith("reader_add_bookmark", { bookId: book.id, position: { trackId: "chapter", offsetMs: 42250 }, note: "" });
    button("Pause").click();
    await vi.waitFor(() => expect(get(privatePlayback).status).toBe("paused"));
    await vi.waitFor(() => expect(button("Resume").disabled).toBe(false));
    flushSync(); button("Resume").click();
    await vi.waitFor(() => expect(get(privatePlayback).status).toBe("playing"));
    await vi.waitFor(() => expect(button("Stop").disabled).toBe(false));
    flushSync(); button("Stop").click();
    await vi.waitFor(() => expect(get(privatePlayback).status).toBe("stopped"));
    expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: book.id, position: { trackId: "chapter", offsetMs: 42250 } });
  });
  it("is mounted outside App's graph/page layout and is never stopped by graph switching", () => {
    const app = readFileSync("src/App.svelte", "utf8");
    expect(app.indexOf("<PrivateReaderToolbar")).toBeLessThan(app.indexOf('<div class="app-layout"'));
    const graphSwitch = app.slice(app.indexOf("function handleGraphChanged()"), app.indexOf("function handleGraphChanged()") + 2000);
    expect(graphSwitch).not.toContain("stopPrivatePlayback");
    expect(graphSwitch).not.toContain("privatePlayback.set");
  });
});
