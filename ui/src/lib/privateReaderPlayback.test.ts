import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { bookmarkPrivatePlayback, checkpointPrivatePlayback, claimPrivateNarration, pausePrivatePlayback, playPrivateAudio, privatePlayback, resumePrivatePlayback, stopPrivatePlayback, updatePrivateNarration, validatePrivateMediaURL } from "./privateReaderPlayback";
import { privateLibrary, type ReaderBook } from "./privateReader";

class FakeAudio extends EventTarget {
  currentTime = 0; preload = ""; src = ""; error = null;
  play = vi.fn(async () => {});
  pause = vi.fn();
  load() { if (this.src) queueMicrotask(() => this.dispatchEvent(new Event("loadedmetadata"))); }
  removeAttribute(name: string) { if (name === "src") this.src = ""; }
}
const elements: FakeAudio[] = [];
const book: ReaderBook = {
  id: "private", title: "Outside graph", kind: "audio", available: true,
  tracks: [{ id: "stable-2", title: "Chapter 2", relativePath: "Disc 1/2.mp3" }, { id: "stable-10", title: "Chapter 10", relativePath: "Disc 1/10.mp3" }],
  position: { trackId: "stable-2", offsetMs: 10000 }, bookmarks: [],
};
beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) => command.endsWith("media_url") ? "http://127.0.0.1:38721/secret/track"
    : command.endsWith("snapshot") ? { libraryPath: "/local", books: [book] } : undefined);
  vi.stubGlobal("Audio", class extends FakeAudio { constructor() { super(); elements.push(this); } });
  privateLibrary.set({ libraryPath: "/local", books: [book] });
});
afterEach(async () => { await stopPrivatePlayback(); vi.unstubAllGlobals(); });

describe("app-owned private playback", () => {
  it("accepts only native loopback transport, never full-file base64 or remote URLs", () => {
    expect(validatePrivateMediaURL("http://127.0.0.1:1234/capability")).toContain("127.0.0.1");
    for (const value of ["https://example.com/audio.mp3", "data:audio/mp3;base64,AAAA", "file:///book.mp3", "http://localhost:1234/book", "http://user@127.0.0.1:1234/book"])
      expect(() => validatePrivateMediaURL(value)).toThrow();
  });
  it("resumes exact track, bookmarks captured current time, and preserves position on stop", async () => {
    await playPrivateAudio(book);
    const audio = elements.at(-1)!;
    expect(audio.currentTime).toBe(10);
    expect(get(privatePlayback).status).toBe("playing");
    audio.currentTime = 18.125;
    await bookmarkPrivatePlayback();
    expect(invoke).toHaveBeenCalledWith("reader_add_bookmark", { bookId: book.id, position: { trackId: "stable-2", offsetMs: 18125 }, note: "" });
    await pausePrivatePlayback();
    expect(get(privatePlayback).status).toBe("paused");
    await resumePrivatePlayback();
    await stopPrivatePlayback();
    expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: book.id, position: { trackId: "stable-2", offsetMs: 18125 } });
    expect(get(privatePlayback).status).toBe("stopped");
  });
  it("never guesses another track when a saved chapter is missing", async () => {
    await expect(playPrivateAudio(book, { trackId: "gone", offsetMs: 200 })).rejects.toThrow("missing");
    expect(invoke).not.toHaveBeenCalledWith("reader_media_url", expect.anything());
  });
  it("uses a single player for narration and audio; native bookmark failure propagates", async () => {
    await playPrivateAudio(book);
    const adapter = { pause: vi.fn(async () => {}), resume: vi.fn(async () => {}), stop: vi.fn(async () => {}), bookmark: vi.fn(async () => { throw new Error("Native save failed"); }) };
    await claimPrivateNarration({ ...book, kind: "epub" }, adapter);
    updatePrivateNarration({ status: "playing" });
    expect(elements.at(-1)!.src).toBe("");
    await pausePrivatePlayback(); expect(adapter.pause).toHaveBeenCalledOnce();
    await resumePrivatePlayback(); expect(adapter.resume).toHaveBeenCalledOnce();
    await expect(bookmarkPrivatePlayback()).rejects.toThrow("Native save failed");
    await playPrivateAudio(book);
    expect(adapter.stop).toHaveBeenCalledOnce();
    expect(get(privatePlayback).mode).toBe("audio");
  });
  it("checkpoints failures are visible to callers and subsequent saves can recover", async () => {
    await playPrivateAudio(book);
    invoke.mockRejectedValueOnce(new Error("Read-only storage"));
    await expect(checkpointPrivatePlayback()).rejects.toThrow("Read-only storage");
    await checkpointPrivatePlayback();
  });
  it("saves the old book's captured position before switching without resetting the new book", async () => {
    const second = { ...book, id: "second-book", title: "Second", position: { trackId: "stable-10", offsetMs: 80000 } };
    privateLibrary.set({ libraryPath: "/local", books: [book, second] });
    await playPrivateAudio(book);
    elements.at(-1)!.currentTime = 42;
    await playPrivateAudio(second);
    expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: book.id, position: { trackId: "stable-2", offsetMs: 42000 } });
    expect(get(privatePlayback).bookId).toBe(second.id);
    expect(elements.at(-1)!.currentTime).toBe(80);
    expect(get(privateLibrary).books.find(item => item.id === second.id)?.position).toEqual(second.position);
  });
  it("ignores an old book's late media response after a different book starts", async () => {
    let finish!: (url: string) => void;
    const delayed = new Promise<string>(resolve => { finish = resolve; });
    invoke.mockImplementation(async (command: string, args: { bookId?: string } = {}) => {
      if (command === "reader_media_url") return args.bookId === book.id ? delayed : "http://127.0.0.1:1234/new-book";
    });
    const old = playPrivateAudio(book);
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_media_url", expect.objectContaining({ bookId: book.id })));
    const second = { ...book, id: "second-book", position: { trackId: "stable-10", offsetMs: 23000 } };
    await playPrivateAudio(second);
    finish("http://127.0.0.1:1234/old-book");
    await old;
    expect(get(privatePlayback).bookId).toBe(second.id);
    expect(elements.at(-1)!.src).toBe("http://127.0.0.1:1234/new-book");
    expect(elements.at(-1)!.currentTime).toBe(23);
  });
  it("does not restart after Stop cancels a pending media request", async () => {
    let finish!: (url: string) => void;
    invoke.mockImplementation(async command => command === "reader_media_url"
      ? new Promise<string>(resolve => { finish = resolve; }) : undefined);
    const opening = playPrivateAudio(book);
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
    await stopPrivatePlayback();
    finish("http://127.0.0.1:1234/cancelled");
    await opening;
    expect(get(privatePlayback).status).toBe("stopped");
    expect(elements.at(-1)?.src ?? "").toBe("");
  });
});
