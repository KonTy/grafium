import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const request = vi.hoisted(() => vi.fn());
vi.mock("./privateReaderAndroid", async importOriginal => ({
  ...await importOriginal<typeof import("./privateReaderAndroid")>(),
  androidReaderRequest: request, isAndroidReader: () => true,
}));
import { canonicalNarrationBatches, registerPrivateSegments, startAndroidPrivateNarration } from "./privateReaderSegments";
import { privateLibrary, type ReaderBook } from "./privateReader";
import { stopPrivatePlayback, privatePlayback } from "./privateReaderPlayback";
import { BOOK_RENDERER_VERSION } from "./bookLocations";
import { get } from "svelte/store";
import { speechPlaybackRate } from "./readerPlaybackPreferences";

const hash = "a".repeat(64);
const locator = { kind: "epub" as const, cfi: "epubcfi(/6/2!/4/2)", rendererVersion: BOOK_RENDERER_VERSION };
const book: ReaderBook = { id: "epub", title: "Private EPUB", available: true, kind: "epub", tracks: [], bookmarks: [], position: null };
const idle = { bookId: null, trackId: null, offsetMs: 0, durationMs: 0, playing: false, buffering: false, error: null };
let unregister = () => {};
beforeEach(() => {
  request.mockReset();
  speechPlaybackRate.set(1);
  request.mockImplementation(async command => {
    if (command === "voiceStatus") return { available: true, selection: { voice_id: "local", language: "ja" } };
    if (command === "narrationBegin") return { uploadId: "transaction", sourceHash: hash };
    if (command === "narrationStart") return { ...idle, bookId: book.id, mode: "tts", playing: true, locator: JSON.stringify(locator) };
    if (command === "stop") return idle;
    return {};
  });
  privateLibrary.set({ libraryPath: "Local documents", books: [book] });
  unregister = registerPrivateSegments(book.id, async () => Array.from({ length: 300 }, (_, index) => ({
    text: `Canonical paragraph ${index}`, locator,
  })), () => hash);
});
afterEach(async () => { await stopPrivatePlayback(); unregister(); });

describe("transactional Android canonical narration", () => {
  it("uploads every segment with ordered ordinals before native commit and Start", async () => {
    await startAndroidPrivateNarration(book.id, true);
    const appends = request.mock.calls.filter(([command]) => command === "narrationAppend");
    expect(appends.map(([, args]) => args.segments.length)).toEqual([256, 44]);
    const segments = appends.flatMap(([, args]) => args.segments);
    expect(segments.map(segment => segment.ordinal)).toEqual(Array.from({ length: 300 }, (_, index) => index));
    expect(segments.every(segment => segment.locator.kind === "epub" && !("source_hash" in segment.locator))).toBe(true);
    expect(request.mock.calls.map(([command]) => command)).toEqual([
      "voiceStatus", "narrationBegin", "narrationAppend", "narrationAppend", "narrationCommit", "narrationStart",
    ]);
    expect(request).toHaveBeenLastCalledWith("narrationStart", { bookId: book.id, fromBeginning: true, playbackRate: 1 });
    unregister();
    expect(get(privatePlayback)).toMatchObject({ mode: "tts", status: "playing" });
  });
  it("passes the selected narration rate without rewriting canonical source segments", async () => {
    speechPlaybackRate.set(4);
    await startAndroidPrivateNarration(book.id);
    expect(request).toHaveBeenLastCalledWith("narrationStart", { bookId: book.id, fromBeginning: false, playbackRate: 4 });
    const segment = request.mock.calls.find(([command]) => command === "narrationAppend")![1].segments[0];
    expect(segment).toEqual({ text: "Canonical paragraph 0", locator, ordinal: 0 });
  });
  it("rejects an EPUB changed since the isolated frame opened before uploading any text", async () => {
    request.mockImplementation(async command => command === "voiceStatus" ? { available: true, selection: {} }
      : command === "narrationBegin" ? { uploadId: "transaction", sourceHash: "b".repeat(64) } : idle);
    await expect(startAndroidPrivateNarration(book.id)).rejects.toThrow("no longer matches");
    expect(request).toHaveBeenCalledWith("narrationCancel", { uploadId: "transaction" });
    expect(request.mock.calls.some(([command]) => command === "narrationAppend")).toBe(false);
  });
  it("cancels a partial upload without committing or starting after Stop", async () => {
    let complete!: () => void;
    request.mockImplementation(async command => {
      if (command === "voiceStatus") return { available: true, selection: {} };
      if (command === "narrationBegin") return { uploadId: "transaction", sourceHash: hash };
      if (command === "narrationAppend") return new Promise<void>(resolve => { complete = resolve; });
      return idle;
    });
    const starting = startAndroidPrivateNarration(book.id);
    const rejected = expect(starting).rejects.toThrow("cancelled");
    await vi.waitFor(() => expect(complete).toBeTypeOf("function"));
    await stopPrivatePlayback();
    complete();
    await rejected;
    expect(request.mock.calls.some(([command]) => command === "narrationCommit" || command === "narrationStart")).toBe(false);
    expect(get(privatePlayback).status).toBe("stopped");
  });
  it.each([
    ["NARRATION_UPLOAD_NOT_FOUND", false],
    ["NATIVE_IO_ERROR", true],
  ])("preserves the preparation error when cleanup reports %s", async (cleanupError, includeCleanupError) => {
    const failure = new Error("SOURCE_CHANGED during commit");
    const defaultRequest = request.getMockImplementation()!;
    request.mockImplementation(async (...args) => {
      if (args[0] === "narrationCommit") throw failure;
      if (args[0] === "narrationCancel") throw new Error(cleanupError);
      return defaultRequest(...args);
    });
    let reported: unknown;
    try { await startAndroidPrivateNarration(book.id); }
    catch (cause) { reported = cause; }
    expect(String(reported)).toContain("SOURCE_CHANGED during commit");
    if (includeCleanupError) {
      expect(String(reported)).toContain(cleanupError);
      expect(get(privatePlayback).error).toContain(cleanupError);
    } else {
      expect(reported).toBe(failure);
    }
    expect(request.mock.calls.some(([command]) => command === "narrationStart")).toBe(false);
  });
  it("counts UTF-8 bytes and rejects invalid or oversize text instead of truncating the book", () => {
    expect(() => canonicalNarrationBatches([{ text: "界".repeat(501), locator }])).toThrow("invalid");
    expect(() => canonicalNarrationBatches([{ text: "valid", locator: { ...locator, cfi: "guessed-offset" } }])).toThrow("invalid");
    const batches = canonicalNarrationBatches(Array.from({ length: 300 }, () => ({ text: "界".repeat(500), locator })));
    expect(batches.every(batch => new TextEncoder().encode(JSON.stringify(batch)).byteLength < 512 * 1024)).toBe(true);
  });
});
