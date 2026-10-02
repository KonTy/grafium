import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { SvelteMap } from "svelte/reactivity";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));
import PrivateReaderLibrary from "./PrivateReaderLibrary.svelte";
import PrivateReaderBook from "./PrivateReaderBook.svelte";
import LibraryMedia from "./LibraryMedia.svelte";
import { privateLibrary, privateLibraryError, type ReaderBook, type ReaderPosition } from "../lib/privateReader";
import { libraryMediaRequest, requestLibraryMedia } from "../lib/library";
import { privatePlayback } from "../lib/privateReaderPlayback";

const source = (extra: Partial<ReaderBook> = {}): ReaderBook => ({
  id: "media", title: "Local video", kind: "video", available: true,
  tracks: [{ id: "track", title: "Video", relativePath: "video.mp4" }],
  position: { trackId: "track", offsetMs: 12000 }, bookmarks: [], ...extra,
});
let component: ReturnType<typeof mount> | undefined;
const button = (name: string) => [...document.querySelectorAll("button")].find(element => element.textContent?.trim() === name)!;
beforeEach(() => {
  invoke.mockReset(); privateLibraryError.set(""); libraryMediaRequest.set(null);
  privatePlayback.set({ bookId: null, title: "", mode: "audio", status: "stopped", position: null, error: "" });
  privateLibrary.set({ libraryPath: "/library", books: [source()] });
  invoke.mockImplementation(async (command, args) => {
    if (command === "reader_media_url") return "http://127.0.0.1:3456/private/video";
    if (command === "study_youtube_embed") return "http://127.0.0.1:3456/private/youtube";
    if (command === "reader_set_favorite") {
      const snapshot = get(privateLibrary);
      return { ...snapshot, books: snapshot.books.map(book => book.id === args.bookId ? { ...book, favorite: args.favorite } : book) };
    }
    if (command === "reader_record_activity") return {
      ...get(privateLibrary), books: get(privateLibrary).books.map(book => book.id === args.bookId
        ? { ...book, lastUsedAt: Date.now(), ...(args.progress ? { progress: args.progress } : {}) } : book),
    };
    if (["reader_snapshot", "reader_rescan"].includes(command)) return get(privateLibrary);
  });
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.restoreAllMocks(); vi.useRealTimers();
  libraryMediaRequest.set(null);
  privatePlayback.set({ bookId: null, title: "", mode: "audio", status: "stopped", position: null, error: "" });
});

describe("Library destination", () => {
  it("opens details without activity, retains the pile, filters favorites, and references Studies", async () => {
    const books = [source(), source({ id: "recent", title: "Recently read", kind: "epub", lastUsedAt: 50, favorite: true })];
    privateLibrary.set({ libraryPath: "/library", books });
    const onOpen = vi.fn(); const onAddToStudies = vi.fn();
    component = mount(PrivateReaderLibrary, { target: document.body, props: { onOpen, onSettings: vi.fn(), onAddToStudies } });
    await vi.waitFor(() => expect(document.querySelectorAll(".book-title")).toHaveLength(2));
    expect(document.querySelector("h1")?.textContent).toBe("Library");
    expect(document.querySelector("section")?.getAttribute("data-help-context")).toBe("library");
    expect([...document.querySelectorAll(".book-title")].map(el => el.textContent)).toEqual(["Recently read", "Local video"]);
    button("Local video").click();
    expect(onOpen).toHaveBeenCalledWith("media");
    expect(invoke).not.toHaveBeenCalledWith("reader_record_activity", expect.anything());
    button("★ Favorites").click(); flushSync();
    expect(document.querySelectorAll(".book-title")).toHaveLength(1);
    button("Add to Studies").click();
    expect(onAddToStudies).toHaveBeenCalledWith(expect.objectContaining({ id: "recent" }));
    expect(invoke.mock.calls.every(([name]) => name.startsWith("reader_"))).toBe(true);
  });
  it("persists favorites without moving the item, and keeps errors visible on failure", async () => {
    component = mount(PrivateReaderLibrary, { target: document.body, props: { onOpen: vi.fn(), onSettings: vi.fn() } });
    await vi.waitFor(() => expect(document.querySelector('[aria-label="Favorite Local video"]')).not.toBeNull());
    document.querySelector<HTMLButtonElement>('[aria-label="Favorite Local video"]')!.click();
    await vi.waitFor(() => expect(get(privateLibrary).books[0].favorite).toBe(true));
    expect(get(privateLibrary).books[0].lastUsedAt).toBe(0);
    await vi.waitFor(() => expect(document.querySelector<HTMLButtonElement>('[aria-label="Unfavorite Local video"]')?.disabled).toBe(false));
    invoke.mockRejectedValueOnce(new Error("Library is read-only"));
    document.querySelector<HTMLButtonElement>('[aria-label="Unfavorite Local video"]')!.click();
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("read-only"));
    expect(get(privateLibrary).books[0].favorite).toBe(true);
  });
  it("rejects nonmedia links in the visible form", async () => {
    component = mount(PrivateReaderLibrary, { target: document.body, props: { onOpen: vi.fn(), onSettings: vi.fn() } });
    flushSync(); button("Add link").click(); flushSync();
    const input = document.querySelector<HTMLInputElement>('input[type="url"]')!;
    input.value = "https://example.test/feed.xml"; input.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("direct audio/video"));
    expect(invoke).not.toHaveBeenCalledWith("reader_add_link", expect.anything());
  });
  it("opens a journal draft only via its bookmark callback, even when source media is missing", () => {
    const book = source({ available: false, bookmarks: [{ id: "mark", bookId: "media", note: "Private reflection", createdAt: 1, position: { trackId: "track", offsetMs: 1000 } }] });
    privateLibrary.set({ libraryPath: "/missing", books: [book] });
    const onJournalNote = vi.fn();
    component = mount(PrivateReaderBook, { target: document.body, props: { bookId: book.id, onBack: vi.fn(), onJournalNote } });
    flushSync();
    expect(document.body.textContent).toContain("Private reflection");
    expect(document.querySelector("video")).toBeNull();
    expect(onJournalNote).not.toHaveBeenCalled();
    button("Journal note…").click();
    expect(onJournalNote).toHaveBeenCalledWith(book, book.bookmarks[0]);
    expect(invoke).not.toHaveBeenCalled();
    expect(document.body.textContent).not.toContain("[[");
  });
  it.each(["audio", "tts"] as const)("reports persistent %s to a Study clock without writes or repeated snapshot activity", mode => {
    const book = source({ kind: mode === "audio" ? "audio" : "epub", available: mode === "audio" });
    const position: ReaderPosition = mode === "audio" ? { trackId: "track", offsetMs: 15000 }
      : { locator: { kind: "epub", cfi: "epubcfi(/6/2!/4/8)", rendererVersion: "fixture" }, offsetMs: 15000 };
    const anchor = mode === "audio" ? "track" : "epubcfi(/6/2!/4/8)";
    privateLibrary.set({ libraryPath: "/library", books: [book] });
    const onPlayback = vi.fn(); const onProgress = vi.fn(); const onActivity = vi.fn();
    component = mount(PrivateReaderBook, { target: document.body, props: { bookId: book.id, onBack: vi.fn(), onPlayback, onProgress, onActivity } });
    flushSync();
    expect(onPlayback).toHaveBeenLastCalledWith(false);
    expect(onActivity).not.toHaveBeenCalled();
    privatePlayback.set({ bookId: book.id, title: book.title, mode, status: "playing", position, error: "" });
    flushSync();
    expect(onPlayback).toHaveBeenLastCalledWith(true);
    expect(onProgress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 15, anchor }));
    expect(onActivity).toHaveBeenCalledOnce();
    privatePlayback.update(state => ({ ...state, position: { ...position, offsetMs: 19000 } }));
    flushSync();
    expect(onProgress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 19 }));
    expect(onActivity).toHaveBeenCalledTimes(2);
    privateLibrary.set({ libraryPath: "/library", books: [{ ...book, favorite: true }] });
    flushSync();
    expect(onActivity).toHaveBeenCalledTimes(2);
    privatePlayback.update(state => ({ ...state, status: "paused" })); flushSync();
    expect(onPlayback).toHaveBeenLastCalledWith(false);
    expect(onActivity).toHaveBeenCalledTimes(2);
    expect(invoke).not.toHaveBeenCalled();
  });
  it("opens an initial video bookmark once rather than silently resuming the book position", async () => {
    const book = source({ bookmarks: [{ id: "target", bookId: "media", note: "", createdAt: 1, position: { trackId: "track", offsetMs: 34000 } }] });
    privateLibrary.set({ libraryPath: "/library", books: [book] });
    const onPlayback = vi.fn(); const onProgress = vi.fn(); const onActivity = vi.fn();
    component = mount(PrivateReaderBook, { target: document.body, props: {
      bookId: book.id, initialBookmarkId: "target", onBack: vi.fn(), onPlayback, onProgress, onActivity,
    } });
    await vi.waitFor(() => expect(document.querySelector("video")).not.toBeNull());
    const video = document.querySelector("video")!;
    Object.defineProperty(video, "readyState", { value: 1 });
    Object.defineProperty(video, "duration", { value: 100 });
    video.dispatchEvent(new Event("loadedmetadata"));
    expect(video.currentTime).toBe(34);
    privateLibrary.set({ libraryPath: "/library", books: [{ ...book, favorite: true }] }); flushSync();
    expect(invoke.mock.calls.filter(([name]) => name === "reader_media_url")).toHaveLength(1);
    video.dispatchEvent(new Event("playing"));
    expect(onPlayback).toHaveBeenLastCalledWith(true);
    expect(onActivity).toHaveBeenCalledOnce();
    video.currentTime = 35; video.dispatchEvent(new Event("timeupdate"));
    expect(onProgress).toHaveBeenLastCalledWith(expect.objectContaining({ position: 35 }));
    video.dispatchEvent(new Event("pause"));
    expect(onPlayback).toHaveBeenLastCalledWith(false);
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_record_activity", expect.anything()));
  });
  it("requires an explicit fallback when the requested bookmark is missing", () => {
    privateLibrary.set({ libraryPath: "/library", books: [source({ kind: "epub", tracks: [] })] });
    component = mount(PrivateReaderBook, { target: document.body, props: { bookId: "media", initialBookmarkId: "deleted", onBack: vi.fn() } });
    flushSync();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("No replacement position was opened");
    expect(document.querySelector("iframe")).toBeNull();
    expect(button("Open current saved place")).toBeDefined();
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe("Library foreground player", () => {
  it("streams registered local video only on Play, records actual movement, and stops on leaving", async () => {
    const book = source();
    component = mount(LibraryMedia, { target: document.body, props: { book } });
    flushSync(); expect(invoke).not.toHaveBeenCalled();
    button("Resume playback").click();
    await vi.waitFor(() => expect(document.querySelector("video")).not.toBeNull());
    expect(invoke).toHaveBeenCalledWith("reader_media_url", { bookId: "media", trackId: "track" });
    expect(invoke).not.toHaveBeenCalledWith("read_asset_data_url", expect.anything());
    const video = document.querySelector("video")!;
    expect(video.src).toBe("http://127.0.0.1:3456/private/video");
    Object.defineProperty(video, "readyState", { value: 1 });
    Object.defineProperty(video, "duration", { value: 100 });
    video.dispatchEvent(new Event("loadedmetadata"));
    expect(video.currentTime).toBe(12);
    expect(invoke).not.toHaveBeenCalledWith("reader_record_activity", expect.anything());
    video.dispatchEvent(new Event("playing"));
    video.currentTime = 15; video.dispatchEvent(new Event("timeupdate"));
    video.dispatchEvent(new Event("pause"));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_record_activity", {
      bookId: "media", progress: expect.objectContaining({ position: 15, total: 100 }),
    }));
    expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: "media", position: { trackId: "track", offsetMs: 15000 } });
    await unmount(component!); component = undefined;
    expect(video.pause).toHaveBeenCalled(); expect(video.getAttribute("src")).toBeNull();
  });
  it("reuses the trusted YouTube wrapper with a saved network bookmark", async () => {
    const book = source({ kind: "youtube", tracks: [], sourceUrl: "https://youtu.be/dQw4w9WgXcQ", position: { offsetMs: 9000 } });
    requestLibraryMedia(book.id, { offsetMs: 24000 });
    component = mount(LibraryMedia, { target: document.body, props: { book } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    expect(invoke).toHaveBeenCalledWith("study_youtube_embed", { videoId: "dQw4w9WgXcQ", start: 24 });
    expect(invoke).not.toHaveBeenCalledWith("reader_media_url", expect.anything());
    expect(document.querySelector("iframe")?.getAttribute("sandbox")).toBe("allow-scripts allow-same-origin");
  });
  it("does not restart a local stream when its private progress snapshot updates", async () => {
    const books = new SvelteMap([["book", source()]]);
    component = mount(LibraryMedia, { target: document.body, props: { get book() { return books.get("book")!; } } });
    flushSync(); button("Resume playback").click();
    await vi.waitFor(() => expect(document.querySelector("video")).not.toBeNull());
    const video = document.querySelector("video");
    books.set("book", source({ lastUsedAt: 100, position: { trackId: "track", offsetMs: 20000 } })); flushSync();
    expect(document.querySelector("video")).toBe(video);
    expect(invoke.mock.calls.filter(([command]) => command === "reader_media_url")).toHaveLength(1);
  });
  it("states the Android local video limitation without requesting bytes", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Android");
    component = mount(LibraryMedia, { target: document.body, props: { book: source() } });
    flushSync();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("not supported on Android");
    expect(invoke).not.toHaveBeenCalled();
  });
});
