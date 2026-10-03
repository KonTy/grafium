import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { applyAndroidState, bookmarkPrivatePlayback, checkpointPrivatePlayback, claimPrivateNarration, pausePrivatePlayback, playPrivateAudio, cuePrivateAudio, privatePlayback, resumePrivatePlayback, seekPrivateAudioPosition, skipPrivateAudio, stopPrivatePlayback, updatePrivateNarration, validatePrivateMediaURL, setPrivatePlaybackRate } from "./privateReaderPlayback";
import { mediaPlaybackRate, speechPlaybackRate } from "./readerPlaybackPreferences";
import { privateLibrary, type ReaderBook } from "./privateReader";

class FakeAudio extends EventTarget {
  playbackRate = 1; defaultPlaybackRate = 1; preservesPitch = false;
  currentTime = 0; preload = ""; src = ""; error = null;
  duration = 120;
  seekable = { length: 1, start: () => 0, end: () => this.duration };
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
  mediaPlaybackRate.set(1); speechPlaybackRate.set(1);
  vi.stubGlobal("localStorage", { setItem: vi.fn(), getItem: vi.fn(() => null) });
  invoke.mockReset();
  invoke.mockImplementation(async (command: string, args) => command.endsWith("media_url") ? "http://127.0.0.1:38721/secret/track"
    : command.endsWith("record_activity") ? {
      ...get(privateLibrary), books: get(privateLibrary).books.map(book => book.id === args.bookId
        ? { ...book, lastUsedAt: Date.now(), ...(args.progress ? { progress: args.progress } : {}) } : book),
    }
    : command.endsWith("snapshot") ? { libraryPath: "/local", books: [book] } : undefined);
  vi.stubGlobal("Audio", class extends FakeAudio { constructor() { super(); elements.push(this); } });
  privateLibrary.set({ libraryPath: "/local", books: [book] });
});
afterEach(async () => { await stopPrivatePlayback(); vi.unstubAllGlobals(); });

describe("app-owned private playback", () => {
  it("applies remembered 4x across chapters and changes speed without resetting bookmarks", async () => {
    mediaPlaybackRate.set(4);
    await playPrivateAudio(book);
    const audio = elements.at(-1)!;
    expect(audio.playbackRate).toBe(4); expect(audio.preservesPitch).toBe(true);
    audio.currentTime = 23.7;
    await pausePrivatePlayback();
    await setPrivatePlaybackRate(2);
    expect(audio.playbackRate).toBe(2); expect(audio.currentTime).toBe(23.7);
    expect(get(privatePlayback)).toMatchObject({ status: "paused", playbackRate: 2 });
    expect(get(speechPlaybackRate)).toBe(1);
    await bookmarkPrivatePlayback();
    expect(invoke).toHaveBeenCalledWith("reader_add_bookmark", expect.objectContaining({ position: { trackId: "stable-2", offsetMs: 23700 } }));
    await playPrivateAudio(book, { trackId: "stable-10", offsetMs: 0 });
    expect(audio.playbackRate).toBe(2); expect(audio.currentTime).toBe(0);
  });
  it("reports actual duration and time, seeks without restarting, and refuses unavailable timelines", async () => {
    await playPrivateAudio(book);
    const audio = elements.at(-1)!;
    expect(get(privatePlayback)).toMatchObject({ durationMs: 120000, seekable: true });
    audio.currentTime = 34.5; audio.dispatchEvent(new Event("timeupdate"));
    expect(get(privatePlayback).position?.offsetMs).toBe(34500);
    await pausePrivatePlayback();
    const plays = audio.play.mock.calls.length;
    await seekPrivateAudioPosition(60000);
    expect(audio.currentTime).toBe(60);
    expect(get(privatePlayback).status).toBe("paused");
    expect(audio.play.mock.calls.length).toBe(plays);
    await skipPrivateAudio(-15000);
    expect(audio.currentTime).toBe(45);
    audio.duration = Infinity; audio.dispatchEvent(new Event("durationchange"));
    expect(get(privatePlayback)).toMatchObject({ durationMs: 0, seekable: false });
    await expect(seekPrivateAudioPosition(10000)).rejects.toThrow("unavailable");
    audio.duration = 120; audio.seekable.length = 0; audio.dispatchEvent(new Event("progress"));
    expect(get(privatePlayback)).toMatchObject({ durationMs: 120000, seekable: false });
    await expect(skipPrivateAudio(15000)).rejects.toThrow("unavailable");
    audio.seekable.length = 1;
  });
  it("keeps native Android duration/seek capability honest and clears it for narration", () => {
    const state = { bookId: book.id, trackId: "stable-2", offsetMs: 1234, playing: true, buffering: false, error: null, durationMs: 120000 };
    applyAndroidState(state);
    expect(get(privatePlayback)).toMatchObject({ durationMs: 120000, seekable: false });
    applyAndroidState({ ...state, seekable: true });
    expect(get(privatePlayback)).toMatchObject({ durationMs: 120000, seekable: true });
    applyAndroidState({ ...state, mode: "tts", locator: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: "test" }, seekable: true });
    expect(get(privatePlayback)).toMatchObject({ durationMs: 0, seekable: false });
  });

  it("sends Android timestamp cues without autoplay", async () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Android");
    const requests: { command: string; args: Record<string, unknown> }[] = [];
    window.PrivateReaderBridge = { request(raw: string) {
      const request = JSON.parse(raw);
      requests.push(request);
      queueMicrotask(() => window.dispatchEvent(new CustomEvent("private-reader-response", { detail: { id: request.id, ok: true, result: {
        bookId: request.args.bookId, trackId: request.args.trackId, offsetMs: request.args.offsetMs, playing: false, buffering: false, error: null, durationMs: 0,
      } } })));
    } };
    await cuePrivateAudio(book, { trackId: "stable-2", offsetMs: 65000 });
    expect(requests.at(-1)).toMatchObject({ command: "play", args: { autoplay: false, offsetMs: 65000 } });
    expect(get(privatePlayback)).toMatchObject({ status: "paused", position: { trackId: "stable-2", offsetMs: 65000 } });
    await stopPrivatePlayback();
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Linux");
    delete window.PrivateReaderBridge;
  });

  it("does not save a cued citation position until playback actually moves", async () => {
    await cuePrivateAudio(book, { trackId: "stable-10", offsetMs: 65000 });
    await stopPrivatePlayback();
    expect(invoke).not.toHaveBeenCalledWith("reader_save_position", expect.objectContaining({
      position: { trackId: "stable-10", offsetMs: 65000 },
    }));
    await cuePrivateAudio(book, { trackId: "stable-10", offsetMs: 65000 });
    await resumePrivatePlayback();
    elements.at(-1)!.currentTime = 66;
    await checkpointPrivatePlayback();
    expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: book.id, position: { trackId: "stable-10", offsetMs: 66000 } });
  });
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
  it("records recency only after listening moves, never from restoration or a paused seek", async () => {
    await playPrivateAudio(book);
    await checkpointPrivatePlayback();
    expect(invoke).not.toHaveBeenCalledWith("reader_record_activity", expect.anything());
    elements.at(-1)!.currentTime = 11;
    await checkpointPrivatePlayback();
    expect(invoke).toHaveBeenCalledWith("reader_record_activity", {
      bookId: book.id, progress: expect.objectContaining({ position: 11, total: 0, anchor: "stable-2" }),
    });
    await pausePrivatePlayback();
    invoke.mockClear();
    elements.at(-1)!.currentTime = 50;
    await checkpointPrivatePlayback();
    expect(invoke).not.toHaveBeenCalledWith("reader_record_activity", expect.anything());
  });
  it("plays explicit network audio without a local track or copying graph assets", async () => {
    const remote: ReaderBook = { ...book, id: "network", tracks: [], sourceUrl: "https://example.test/audio.mp3", position: { offsetMs: 3000 } };
    privateLibrary.set({ libraryPath: null, books: [remote] });
    await playPrivateAudio(remote);
    expect(elements.at(-1)!.src).toBe(remote.sourceUrl);
    expect(elements.at(-1)!.currentTime).toBe(3);
    expect(invoke).not.toHaveBeenCalledWith("reader_media_url", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("read_asset_data_url", expect.anything());
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
  it("cancels pending metadata without waiting for timeout or restarting after Stop", async () => {
    await playPrivateAudio(book);
    const audio = elements.at(-1)!;
    const load = vi.spyOn(audio, "load").mockImplementation(() => {});
    const opening = playPrivateAudio(book);
    try {
      await vi.waitFor(() => expect(get(privatePlayback).status).toBe("loading"));
      await stopPrivatePlayback();
      await opening;
      expect(get(privatePlayback)).toMatchObject({ status: "stopped", error: "" });
      expect(audio.src).toBe("");
    } finally { load.mockRestore(); }
  });
});
