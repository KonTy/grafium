import { beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { bookmarkDate, bookmarkLabel, bookmarkExcerpt, compactBookmarkLabel, privateLibrary, privateLibraryError, readerNative, readerTime, refreshPrivateLibrary, savePrivateBookmark, savePrivatePosition, type ReaderBook } from "./privateReader";
import { readReaderMessage } from "./bookReaderSecurity";

const book: ReaderBook = {
  id: "private", title: "External book", kind: "audio", available: true,
  tracks: [{ id: "track-2", title: "Chapter 2", relativePath: "Disc 1/2.mp3" }],
  position: null, bookmarks: [],
};
beforeEach(() => {
  invoke.mockReset();
  privateLibrary.set({ libraryPath: "/library", books: [book] });
  privateLibraryError.set("");
});
describe("private reader persistence boundary", () => {
  it("makes bounded Unicode labels without rewriting saved private notes", () => {
    expect(bookmarkExcerpt("  “Слово второе”, then more words")).toBe("Слово второе");
    expect(bookmarkExcerpt("One")).toBe("One");
    expect(bookmarkExcerpt("!!!")).toBe("");
    expect(Array.from(bookmarkExcerpt("é".repeat(200)))).toHaveLength(80);
    const mark = { id: "mark", bookId: book.id, createdAt: 1, note: "  Existing\nlong private comment  ", position: { trackId: "track-2", offsetMs: 0 } };
    expect(compactBookmarkLabel(book, mark)).toBe("Existing long private comment");
    expect(mark.note).toBe("  Existing\nlong private comment  ");
    expect(compactBookmarkLabel(book, { ...mark, note: " " })).toBe("Chapter 2 · 0:00");
  });
  it("saves native app-level positions without graph identifiers", async () => {
    invoke.mockResolvedValue(undefined);
    const position = { trackId: "track-2", offsetMs: 12345 };
    await savePrivatePosition(book.id, position);
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: book.id, position });
    expect(get(privateLibrary).books[0].position).toEqual(position);
  });
  it("does not claim failed writes or bookmarks succeeded", async () => {
    invoke.mockRejectedValue(new Error("Disk full"));
    await expect(savePrivatePosition(book.id, { offsetMs: 10 })).rejects.toThrow("Disk full");
    expect(get(privateLibrary).books[0].position).toBeNull();
    await expect(savePrivateBookmark(book.id, { offsetMs: 10 })).rejects.toThrow("Disk full");
    expect(get(privateLibrary).books[0].bookmarks).toEqual([]);
  });
  it("ignores a stale rescan result and retains missing-source history", async () => {
    let finish!: (value: unknown) => void;
    invoke.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    const old = refreshPrivateLibrary(true);
    const unavailable = { ...book, available: false };
    invoke.mockResolvedValueOnce({ libraryPath: "/new", books: [unavailable] });
    await refreshPrivateLibrary();
    finish({ libraryPath: "/old", books: [] });
    await old;
    expect(get(privateLibrary)).toEqual({ libraryPath: "/new", books: [{ ...unavailable, favorite: false, lastUsedAt: 0 }] });
  });
  it("keeps discovery errors visible without destroying prior records", async () => {
    invoke.mockRejectedValue(new Error("Grant revoked"));
    await expect(refreshPrivateLibrary(true)).rejects.toThrow("Grant revoked");
    expect(get(privateLibrary).books).toEqual([book]);
    expect(get(privateLibraryError)).toContain("Grant revoked");
  });
  it("uses stable track IDs and formats long audiobook positions", () => {
    expect(bookmarkLabel(book, { trackId: "track-2", offsetMs: 3723000 })).toBe("Chapter 2 · 1:02:03");
    expect(bookmarkLabel(book, { trackId: "gone", offsetMs: 2000 })).toContain("Unavailable chapter");
    expect(readerTime(0)).toBe("0:00");
  });
  it("serializes actual native relink, note, export, and restore contracts without guessed prefixes", async () => {
    const backup = JSON.stringify({ schemaVersion: 1, books: [] });
    invoke.mockResolvedValue(backup);
    const relink = { bookId: book.id, relativePath: "Moved book", confirmReplacement: true };
    await readerNative("relink", relink);
    await readerNative("update_bookmark", { bookId: book.id, bookmarkId: "mark", note: "Private note" });
    expect(await readerNative("export")).toBe(backup);
    await readerNative("restore", { data: backup });
    expect(JSON.parse(JSON.stringify(invoke.mock.calls))).toEqual([
      ["reader_relink", relink],
      ["reader_update_bookmark", { bookId: book.id, bookmarkId: "mark", note: "Private note" }],
      ["reader_export", null],
      ["reader_restore", { backup }],
    ]);
  });
  it("renders native bookmark epoch milliseconds as dates rather than seconds", () => {
    expect(bookmarkDate(1760000000000)).toBe("2025-10-09T08:53:20.000Z");
    expect(bookmarkDate("2025-10-09T08:53:20.000Z")).toBe("2025-10-09T08:53:20.000Z");
    expect(bookmarkDate(Number.MAX_VALUE)).toBe("");
  });
});

describe("isolated read-aloud segment validation", () => {
  const token = "mount-token";
  const source = window;
  const valid = {
    channel: "grafium-book", token, type: "read-aloud-segments", requestId: "request-1",
    section: 0, sectionCount: 2, nextOffset: null,
    segments: [{ text: "Only local text", locator: { kind: "epub", cfi: "epubcfi(/6/2!/4/2)", rendererVersion: "v1" } }],
  };
  const event = (data: unknown, origin = "null") => new MessageEvent("message", { data, source, origin });
  it("accepts bounded segment batches with stable EPUB locators", () => {
    expect(readReaderMessage(event(valid), source, token)).toEqual(valid);
  });
  it("requires opaque origin, exact frame source, and per-mount token", () => {
    expect(readReaderMessage(event(valid, "https://attacker.test"), source, token)).toBeNull();
    expect(readReaderMessage(event(valid), null, token)).toBeNull();
    expect(readReaderMessage(event(valid), source, "different")).toBeNull();
  });
  it("rejects oversized text, bad locators, and invalid spine indexes", () => {
    expect(readReaderMessage(event({ ...valid, section: 2 }), source, token)).toBeNull();
    expect(readReaderMessage(event({ ...valid, segments: [{ text: "a".repeat(2049), locator: valid.segments[0].locator }] }), source, token)).toBeNull();
    expect(readReaderMessage(event({ ...valid, segments: [{ text: "text", locator: { kind: "epub", cfi: "javascript:bad", rendererVersion: "v1" } }] }), source, token)).toBeNull();
    expect(readReaderMessage(event({ ...valid, segments: Array(129).fill(valid.segments[0]) }), source, token)).toBeNull();
  });
  it("accepts validated metadata language suggestions without a fixed language shortlist", () => {
    const ready = { channel: "grafium-book", token, type: "ready", toc: [], annotations: true, notice: "" };
    expect(readReaderMessage(event({ ...ready, language: "zh-Hant-HK" }), source, token)).toMatchObject({ language: "zh-Hant-HK" });
    expect(readReaderMessage(event({ ...ready, language: "x-local-voice" }), source, token)).toMatchObject({ language: "x-local-voice" });
    expect(readReaderMessage(event({ ...ready, language: "<script>" }), source, token)).toBeNull();
    expect(readReaderMessage(event({ ...ready, language: "a".repeat(64) }), source, token)).toBeNull();
  });
});
