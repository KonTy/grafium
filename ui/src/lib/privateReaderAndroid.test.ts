import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { androidPrivateCommand, androidReaderRequest, normalizeAndroidReaderPosition } from "./privateReaderAndroid";
import { applyAndroidState, attachPrivatePlayback, bookmarkPrivatePlayback, checkpointPrivatePlayback, playPrivateAudio, privatePlayback, seekPrivateAudioPosition, stopPrivatePlayback } from "./privateReaderPlayback";
import { privateLibrary, type ReaderBook } from "./privateReader";
import { get } from "svelte/store";
import { BOOK_RENDERER_VERSION } from "./bookLocations";
import sharedPosition from "../../tests/fixtures/private-reader-position.json";

const book: ReaderBook = { id: "native", title: "SAF book", available: true, kind: "audio", tracks: [
  { id: "doc-1", title: "1.mp3", relativePath: "Disc 1/1.mp3" },
], position: { trackId: "doc-1", offsetMs: 3500 }, bookmarks: [] };
let requests: { id: string; command: string; args: Record<string, unknown> }[] = [];
beforeEach(() => {
  requests = [];
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Android");
  privateLibrary.set({ libraryPath: "Local documents", books: [book] });
  window.PrivateReaderBridge = { request(json) {
    const request = JSON.parse(json);
    requests.push(request);
    const result = request.command === "library" ? { configured: true, locationLabel: "Local documents", books: [book] }
      : request.command === "stop" ? { bookId: null, trackId: null, offsetMs: 3500, playing: false, buffering: false, error: null }
      : { bookId: book.id, trackId: "doc-1", offsetMs: 3500, playing: true, buffering: false, error: null };
    queueMicrotask(() => window.dispatchEvent(new CustomEvent("private-reader-response", { detail: { id: request.id, ok: true, result } })));
  } };
});
afterEach(async () => {
  await stopPrivatePlayback();
  delete window.PrivateReaderBridge;
  vi.restoreAllMocks();
});
describe("Android private reader bridge", () => {
  it("restores background service controls and seeks only when native capability is reported", async () => {
    const detach = attachPrivatePlayback();
    try {
      await vi.waitFor(() => expect(requests.some(request => request.command === "state")).toBe(true));
      await vi.waitFor(() => expect(get(privatePlayback)).toMatchObject({ bookId: book.id, status: "playing", seekable: false }));
      await expect(seekPrivateAudioPosition(20000)).rejects.toThrow("unavailable");
      window.dispatchEvent(new CustomEvent("private-reader-state", { detail: {
        bookId: book.id, trackId: "doc-1", offsetMs: 3500, durationMs: 90000, seekable: true,
        playing: false, buffering: false, error: null,
      } }));
      expect(get(privatePlayback)).toMatchObject({ status: "paused", durationMs: 90000, seekable: true });
      await seekPrivateAudioPosition(20000);
      expect(requests.find(request => request.command === "seek")?.args).toEqual({ offsetMs: 20000 });
      await stopPrivatePlayback();
      expect(get(privatePlayback)).toMatchObject({ status: "stopped", bookId: null, seekable: false });
      expect(requests.some(request => request.command === "media_url")).toBe(false);
    } finally { detach(); }
  });
  it("restores service Stop controls even while the Library refresh is stalled", async () => {
    const bridge = window.PrivateReaderBridge!;
    let libraryRequest = "";
    window.PrivateReaderBridge = { request(json) {
      const request = JSON.parse(json);
      if (request.command === "library") libraryRequest = request.id;
      else bridge.request(json);
    } };
    privateLibrary.set({ libraryPath: null, books: [] });
    const detach = attachPrivatePlayback();
    try {
      await vi.waitFor(() => expect(get(privatePlayback)).toMatchObject({ bookId: book.id, status: "playing" }));
      await stopPrivatePlayback();
      expect(requests.some(request => request.command === "stop")).toBe(true);
    } finally {
      detach();
      window.dispatchEvent(new CustomEvent("private-reader-response", {
        detail: { id: libraryRequest, ok: false, error: "Library unavailable" },
      }));
    }
  });
  it("controls native playback and captures bookmarks in the service, not at a stale UI offset", async () => {
    await playPrivateAudio(book);
    expect(requests.find(request => request.command === "play")?.args).toEqual({ bookId: book.id, trackId: "doc-1", offsetMs: 3500 });
    expect(get(privatePlayback).status).toBe("playing");
    await bookmarkPrivatePlayback();
    expect(requests.find(request => request.command === "bookmark")?.args).toEqual({ bookId: book.id });
    expect(requests.some(request => request.command === "media_url")).toBe(false);
  });
  it("normalizes SAF library without coercing provider IDs into filesystem paths", async () => {
    const result = await androidPrivateCommand("snapshot");
    expect(result).toEqual({ libraryPath: "Local documents", books: [book] });
  });
  it("rejects native write failures instead of showing a saved bookmark", async () => {
    window.PrivateReaderBridge = { request(json) {
      const { id } = JSON.parse(json);
      queueMicrotask(() => window.dispatchEvent(new CustomEvent("private-reader-response", { detail: { id, ok: false, error: "PRIVATE_STATE_WRITE_FAILED" } })));
    } };
    await expect(androidReaderRequest("bookmark")).rejects.toThrow("PRIVATE_STATE_WRITE_FAILED");
    privatePlayback.update(state => ({ ...state, bookId: null }));
  });
  it("does not silently use browser playback when the native bridge is unavailable", async () => {
    delete window.PrivateReaderBridge;
    await expect(androidReaderRequest("play")).rejects.toThrow("native Android private reader bridge is unavailable");
    privatePlayback.update(state => ({ ...state, bookId: null }));
  });
  it("keeps service-owned narration distinct from audio and captures native EPUB bookmarks", async () => {
    const cfi = "epubcfi(/6/2!/4/2)";
    applyAndroidState({ bookId: book.id, trackId: null, offsetMs: 1275, durationMs: 20000,
      playing: true, buffering: false, error: null, mode: "tts", ttsLoading: false,
      ordinal: 4, segmentCount: 10, locator: { kind: "epub", cfi, rendererVersion: BOOK_RENDERER_VERSION } });
    expect(get(privatePlayback)).toMatchObject({
      mode: "tts", status: "playing", position: { offsetMs: 1275, locator: { kind: "epub", cfi, rendererVersion: BOOK_RENDERER_VERSION } },
    });
    await checkpointPrivatePlayback();
    expect(requests).toEqual([]);
    await bookmarkPrivatePlayback();
    expect(requests.find(request => request.command === "bookmark")?.args).toEqual({ bookId: book.id });
    expect(requests.some(request => request.command === "position")).toBe(false);
  });
  it("normalizes native narration history without silently replacing an existing renderer version", () => {
    const cfi = "epubcfi(/6/2!/4/2)";
    expect(normalizeAndroidReaderPosition({ offsetMs: 700, locator: JSON.stringify({ kind: "epub", cfi, rendererVersion: BOOK_RENDERER_VERSION }) }))
      .toEqual({ offsetMs: 700, locator: { kind: "epub", cfi, rendererVersion: BOOK_RENDERER_VERSION } });
    expect(normalizeAndroidReaderPosition({ offsetMs: 700, locator: { kind: "epub", cfi, rendererVersion: "older-renderer" } })?.locator)
      .toMatchObject({ rendererVersion: "older-renderer" });
    expect(() => normalizeAndroidReaderPosition({ offsetMs: 0, locator: '{"broken"' })).toThrow("invalid");
    expect(() => normalizeAndroidReaderPosition({ offsetMs: 0, locator: { cfi, source_hash: "native-source" } })).toThrow("Unsupported");
    expect(() => normalizeAndroidReaderPosition({ offsetMs: 0, locator: cfi })).toThrow("Unsupported");
  });
  it("sends complete canonical locator objects for visual-reading persistence", async () => {
    await androidPrivateCommand("save_position", { bookId: book.id, position: sharedPosition });
    expect(requests.find(request => request.command === "position")?.args).toEqual({
      bookId: book.id, locator: sharedPosition.locator, offsetMs: 2370,
    });
  });
  it("bookmarks the selected visual range without saving position or calling the service bookmark", async () => {
    const position = { offsetMs: 0, locator: { ...sharedPosition.locator, cfi: "epubcfi(/6/4!/4/2,/1:4,/1:19)" } };
    const args = { bookId: book.id, position, note: "Selected passage" };
    await androidPrivateCommand("bookmark", args);
    expect(requests.map(({ command, args }) => ({ command, args }))).toEqual([
      { command: "bookmarkVisual", args },
    ]);
  });
  it("maps confirmed bookmark deletion without a playback command", async () => {
    const args = { bookId: book.id, bookmarkId: "selected-mark" };
    await androidPrivateCommand("delete_bookmark", args);
    expect(requests.map(({ command, args }) => ({ command, args }))).toEqual([
      { command: "deleteBookmark", args },
    ]);
  });
  it("surfaces visual bookmark and deletion errors without service or position fallbacks", async () => {
    window.PrivateReaderBridge = { request(json) {
      const request = JSON.parse(json);
      requests.push(request);
      queueMicrotask(() => window.dispatchEvent(new CustomEvent("private-reader-response", {
        detail: { id: request.id, ok: false, error: "PRIVATE_STATE_WRITE_FAILED" },
      })));
    } };
    await expect(androidPrivateCommand("bookmark", { bookId: book.id, position: sharedPosition, note: "Selected words" }))
      .rejects.toThrow("PRIVATE_STATE_WRITE_FAILED");
    await expect(androidPrivateCommand("delete_bookmark", { bookId: book.id, bookmarkId: "selected-mark" }))
      .rejects.toThrow("PRIVATE_STATE_WRITE_FAILED");
    expect(requests.map(request => request.command)).toEqual(["bookmarkVisual", "deleteBookmark"]);
  });
});
