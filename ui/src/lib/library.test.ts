import { beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { libraryBooks, libraryLink, libraryPercent, saveLibraryCheckpoint } from "./library";
import { privateLibrary, type ReaderBook } from "./privateReader";

const book = (id: string, extra: Partial<ReaderBook> = {}): ReaderBook => ({
  id, title: id, kind: "video", available: true, tracks: [{ id: "track", title: "Video", relativePath: "video.mp4" }],
  position: null, bookmarks: [], ...extra,
});
beforeEach(() => {
  invoke.mockReset();
  privateLibrary.set({ libraryPath: "/library", books: [book("one")] });
  invoke.mockImplementation(async (command, args) => command === "reader_record_activity" ? {
    ...get(privateLibrary), books: get(privateLibrary).books.map(book => book.id === args.bookId
      ? { ...book, lastUsedAt: Date.now(), ...(args.progress ? { progress: args.progress } : {}) } : book),
  } : undefined);
});
describe("Library projections", () => {
  it("sorts only durable activity, with stable title/id fallback and no mutation", () => {
    const books = [book("b"), book("a", { favorite: true }), book("recent", { lastUsedAt: 42 })];
    expect(libraryBooks(books).map(book => book.id)).toEqual(["recent", "a", "b"]);
    expect(books.map(book => book.id)).toEqual(["b", "a", "recent"]);
    expect(libraryBooks(books, "", true).map(book => book.id)).toEqual(["a"]);
    expect(libraryBooks(books, "REC", false, "video").map(book => book.id)).toEqual(["recent"]);
    expect(libraryBooks(books, "", false, "epub")).toEqual([]);
  });
  it("does not guess completion when a duration is unknown", () => {
    expect(libraryPercent(book("unknown", { progress: { position: 100, total: 0, label: "Chapter 2", anchor: "" } }))).toBeNull();
    expect(libraryPercent(book("known", { progress: { position: 50, total: 100, label: "", anchor: "" } }))).toBe(50);
  });
  it("validates network links with shared media detection and safe title fallbacks", () => {
    expect(libraryLink("https://youtu.be/dQw4w9WgXcQ")).toMatchObject({ kind: "youtube", url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ" });
    expect(libraryLink("https://example.test/My_video.mp4")).toEqual({ kind: "video", title: "My video", url: "https://example.test/My_video.mp4" });
    expect(libraryLink("https://example.test/stream", "My recording", "audio")).toMatchObject({ kind: "audio", title: "My recording" });
    for (const source of ["file:///video.mp4", "https://user:secret@example.test/a.mp3", "https://example.test/feed.xml", "https://example.test/page", "https://youtu.be/short"])
      expect(() => libraryLink(source)).toThrow();
  });
});
describe("Library checkpoint queue", () => {
  it("does not record activity for passive restoration", async () => {
    await saveLibraryCheckpoint("one", { trackId: "track", offsetMs: 1000 }, undefined, false);
    expect(invoke).toHaveBeenCalledWith("reader_save_position", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("reader_record_activity", expect.anything());
  });
  it("serializes position/activity pairs and recovers after persistence failure", async () => {
    let resolve!: () => void;
    invoke.mockImplementationOnce(() => new Promise<void>(done => { resolve = done; }));
    const first = saveLibraryCheckpoint("one", { trackId: "track", offsetMs: 1000 });
    const second = saveLibraryCheckpoint("one", { trackId: "track", offsetMs: 2000 });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
    resolve(); await Promise.all([first, second]);
    expect(invoke.mock.calls.map(([command]) => command)).toEqual([
      "reader_save_position", "reader_record_activity", "reader_save_position", "reader_record_activity",
    ]);
    invoke.mockRejectedValueOnce(new Error("Disk full"));
    await expect(saveLibraryCheckpoint("one", { offsetMs: 3000 })).rejects.toThrow("Disk full");
    await saveLibraryCheckpoint("one", { offsetMs: 4000 });
    expect(get(privateLibrary).books[0].position?.offsetMs).toBe(4000);
  });
});
