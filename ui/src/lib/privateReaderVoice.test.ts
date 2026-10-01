import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { boundedVoiceAudio, startPrivateReadAloud, voiceCommand } from "./privateReaderVoice";
import { privateLibrary } from "./privateReader";
import { privatePlayback, stopPrivatePlayback, pausePrivatePlayback, resumePrivatePlayback, bookmarkPrivatePlayback } from "./privateReaderPlayback";
import { registerPrivateSegments } from "./privateReaderSegments";
import { BOOK_RENDERER_VERSION } from "./bookLocations";
import type { ReaderTextSegment } from "./bookReaderSecurity";

class FakeAudio extends EventTarget {
  currentTime = 0; duration = 20; preload = ""; src = ""; error = null;
  play = vi.fn(async () => {}); pause = vi.fn();
  load() { if (this.src) queueMicrotask(() => this.dispatchEvent(new Event("loadedmetadata"))); }
  removeAttribute(name: string) { if (name === "src") this.src = ""; }
}
const elements: FakeAudio[] = [];
const segment = (ordinal: number): ReaderTextSegment => ({
  text: `Canonical chunk ${ordinal}.`,
  locator: { kind: "epub", rendererVersion: BOOK_RENDERER_VERSION,
    cfi: `epubcfi(/6/2!/4/2/1,:${ordinal * 320},:${(ordinal + 1) * 320})` },
});
let releaseSegments = () => {};
function wave() {
  const value = new ArrayBuffer(44);
  const bytes = new Uint8Array(value);
  bytes.set(new TextEncoder().encode("RIFF")); bytes.set(new TextEncoder().encode("WAVE"), 8);
  return value;
}
beforeEach(() => {
  invoke.mockReset(); elements.length = 0;
  vi.stubGlobal("Audio", class extends FakeAudio { constructor() { super(); elements.push(this); } });
  vi.stubGlobal("speechSynthesis", { speak: vi.fn(() => { throw new Error("Forbidden system speech"); }) });
  URL.createObjectURL = vi.fn(() => "blob:private-voice");
  URL.revokeObjectURL = vi.fn();
  privateLibrary.set({ libraryPath: "/local", books: [{
    id: "book", title: "Private EPUB", kind: "epub", available: true, tracks: [], position: null, bookmarks: [],
  }] });
  releaseSegments = registerPrivateSegments("book", async () => [0, 1, 2].map(segment));
  invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
    if (command === "private_voice_status") return { available: true, selection: { voice_id: "voice", language: "fr-CA" } };
    if (command === "private_voice_synthesize") return { file_name: "clip.wav" };
    if (command === "private_voice_audio") return wave();
    if (command === "reader_snapshot") return get(privateLibrary);
    if (command === "reader_add_bookmark") return { id: "mark", ...args };
  });
});
afterEach(async () => {
  await stopPrivatePlayback(); releaseSegments();
  vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); delete window.PrivateReaderBridge;
});

describe("native private narration", () => {
  it("uses exact renderer chunks and durably resumes the same voice's saved clip offset", async () => {
    privateLibrary.update(snapshot => ({ ...snapshot, books: snapshot.books.map(book => ({
      ...book, position: { locator: segment(1).locator, offsetMs: 1500, voiceId: "voice" },
    })) }));
    await startPrivateReadAloud("book");
    expect(invoke).toHaveBeenCalledWith("private_voice_synthesize", expect.objectContaining({ text: segment(1).text }));
    expect(elements[0].currentTime).toBe(1.5);
    expect(get(privatePlayback).mode).toBe("tts");
    expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: "book",
      position: { locator: segment(1).locator, offsetMs: 1500, voiceId: "voice" } });
    elements[0].currentTime = 2.37;
    await pausePrivatePlayback(); await resumePrivatePlayback();
    expect(get(privateLibrary).books[0].position).toEqual({ locator: segment(1).locator, offsetMs: 2370, voiceId: "voice" });
    await bookmarkPrivatePlayback();
    expect(invoke).toHaveBeenCalledWith("reader_add_bookmark", { bookId: "book", note: "",
      position: { locator: segment(1).locator, offsetMs: 2370, voiceId: "voice" } });
    expect(elements[0].play).toHaveBeenCalledTimes(2);
    expect(window.speechSynthesis.speak).not.toHaveBeenCalled();
  });

  it("stops synthesis on mode changes and keeps at most one clip prefetched", async () => {
    await startPrivateReadAloud("book", true);
    await stopPrivatePlayback();
    expect(invoke).toHaveBeenCalledWith("private_voice_cancel", undefined);
    expect(invoke.mock.calls.some(([command]) => ["private_voice_prepare", "private_voice_segment", "private_voice_checkpoint"].includes(command))).toBe(false);
    const segments = invoke.mock.calls.filter(([command]) => command === "private_voice_synthesize");
    expect(segments.length).toBeLessThanOrEqual(2);
  });

  it("surfaces missing runtimes without invoking any speech fallback", async () => {
    invoke.mockImplementation(async (command: string) => command === "private_voice_status"
      ? { available: false, reason: "Piper is missing", selection: null } : undefined);
    await expect(startPrivateReadAloud("book")).rejects.toThrow("Piper is missing");
    expect(invoke.mock.calls.some(([command]) => command === "private_voice_synthesize")).toBe(false);
    expect(window.speechSynthesis.speak).not.toHaveBeenCalled();
  });

  it("validates bounded audio", () => {
    expect(boundedVoiceAudio(wave()).byteLength).toBe(44);
    for (const value of [new ArrayBuffer(2), new ArrayBuffer(44), "https://example.org/audio"])
      expect(() => boundedVoiceAudio(value)).toThrow();
  });
  it.each(["different-voice", undefined])("resets within-clip timing for stored voice %s", async voiceId => {
    privateLibrary.update(snapshot => ({ ...snapshot, books: snapshot.books.map(book => ({
      ...book, position: { locator: segment(2).locator, offsetMs: 9000, voiceId },
    })) }));
    await startPrivateReadAloud("book");
    expect(elements[0].currentTime).toBe(0);
    expect(get(privateLibrary).books[0].position).toEqual({ locator: segment(2).locator, offsetMs: 0, voiceId: "voice" });
  });
  it("rejects obsolete or nonsegment positions instead of guessing a paragraph", async () => {
    await expect(startPrivateReadAloud("book", { kind: "epub", cfi: "epubcfi(/6/2!/4/2)", rendererVersion: "grafium-native-spine-v1" }))
      .rejects.toThrow("exact narration segment");
    expect(invoke.mock.calls.some(([command]) => command === "private_voice_synthesize")).toBe(false);
    await startPrivateReadAloud("book", true);
    expect(elements[1].currentTime).toBe(0);
  });
  it("rejects ambiguous duplicate chunk CFIs", async () => {
    releaseSegments();
    releaseSegments = registerPrivateSegments("book", async () => [segment(0), segment(0)]);
    await expect(startPrivateReadAloud("book", true)).rejects.toThrow("distinct canonical reader CFIs");
  });
  it("checkpoints each distinct chunk and continues after the reader unmounts", async () => {
    await startPrivateReadAloud("book", true);
    releaseSegments();
    elements[0].dispatchEvent(new Event("ended"));
    await vi.waitFor(() => expect(get(privateLibrary).books[0].position?.locator).toEqual(segment(1).locator));
    expect(new Set([0, 1, 2].map(i => JSON.stringify(segment(i).locator))).size).toBe(3);
  });
  it("writes canonical voice-aware progress to library history every four seconds", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    await startPrivateReadAloud("book", true);
    elements[0].currentTime = 4.125;
    await vi.advanceTimersByTimeAsync(4000);
    expect(invoke).toHaveBeenLastCalledWith("reader_save_position", { bookId: "book",
      position: { locator: segment(0).locator, offsetMs: 4125, voiceId: "voice" } });
    expect(get(privateLibrary).books[0].position?.offsetMs).toBe(4125);
    expect(get(privatePlayback).position?.locator).toEqual(segment(0).locator);
  });
  it("routes Android narration and voice actions exclusively through the native canonical queue", async () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Android");
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    const hash = "a".repeat(64);
    const locator = { kind: "epub" as const, cfi: "epubcfi(/6/2!/4/2)", rendererVersion: BOOK_RENDERER_VERSION };
    const release = registerPrivateSegments("book", async () => [{ text: "Native canonical narration", locator }], () => hash);
    window.PrivateReaderBridge = { request(raw) {
      const request = JSON.parse(raw); calls.push(request);
      const result = request.command === "voiceStatus" ? { available: true, selection: { voice_id: "native", language: "en" }, installed: [] }
        : request.command === "narrationBegin" ? { uploadId: "queue", sourceHash: hash }
        : request.command === "narrationStart" ? { bookId: "book", mode: "tts", playing: true, buffering: false, offsetMs: 0, locator }
        : request.command === "stop" ? { bookId: null, playing: false, buffering: false } : {};
      queueMicrotask(() => window.dispatchEvent(new CustomEvent("private-reader-response", { detail: { id: request.id, ok: true, result } })));
    } };
    try {
      await startPrivateReadAloud("book", true);
      expect(calls.find(call => call.command === "narrationStart")?.args).toEqual({ bookId: "book", fromBeginning: true });
      expect(calls.some(call => call.command === "narrationPrepare")).toBe(false);
      expect(elements).toHaveLength(0);
      expect(invoke).not.toHaveBeenCalled();
      expect(window.speechSynthesis.speak).not.toHaveBeenCalled();
      await voiceCommand("import");
      await voiceCommand("select", { voiceId: "native", language: "en" });
      await expect(voiceCommand("download", { manifest: {}, userAuthorized: false })).rejects.toThrow("authorization");
      await voiceCommand("download", { manifest: { id: "native" }, userAuthorized: true });
      expect(calls.find(call => call.command === "importVoice")?.args).toEqual({});
      expect(calls.find(call => call.command === "selectVoice")?.args).toEqual({ voiceId: "native", language: "en" });
      expect(calls.find(call => call.command === "downloadVoice")?.args).toEqual({ manifest: { id: "native" }, authorized: true });
    } finally { release(); }
  });
});
