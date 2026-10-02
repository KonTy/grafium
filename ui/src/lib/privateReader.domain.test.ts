import { beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import {
  addPrivateLibraryLink, normalizePrivateLibraryLink, privateLibrary, recordPrivateLibraryActivity,
  refreshPrivateLibrary, savePrivatePosition, setPrivateFavorite, type ReaderBook,
} from "./privateReader";

const book: ReaderBook = { id: "video", title: "Talk", kind: "video", sourceUrl: "https://media.example/talk.mp4",
  available: true, tracks: [], position: null, bookmarks: [], favorite: false, lastUsedAt: 0 };
const snapshot = (value = book) => ({ libraryPath: null, books: [value] });
beforeEach(() => { invoke.mockReset(); privateLibrary.set(snapshot()); });

describe("app-private Library contract", () => {
  it("persists favorites without recording activity or graph identifiers", async () => {
    invoke.mockResolvedValue(snapshot({ ...book, favorite: true }));
    await setPrivateFavorite(book.id, true);
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("reader_set_favorite", { bookId: book.id, favorite: true });
    expect(get(privateLibrary).books[0]).toMatchObject({ favorite: true, lastUsedAt: 0 });
  });

  it("records validated activity only through its explicit command", async () => {
    const progress = { position: 20, total: 120, anchor: "", label: "0:20" };
    invoke.mockResolvedValue(snapshot({ ...book, lastUsedAt: 1000, progress }));
    await recordPrivateLibraryActivity(book.id, progress);
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("reader_record_activity", { bookId: book.id, progress });
    expect(get(privateLibrary).books[0].progress).toEqual(progress);
    for (const bad of [NaN, Infinity, -1, 121]) {
      await expect(recordPrivateLibraryActivity(book.id, { ...progress, position: bad })).rejects.toThrow();
    }
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("adds normalized video links and returns the native stable item ID", async () => {
    invoke.mockResolvedValue(snapshot());
    expect(await addPrivateLibraryLink("Talk", "video", book.sourceUrl!)).toEqual(book);
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("reader_add_link",
      { title: "Talk", kind: "video", url: book.sourceUrl });
  });

  it("retains state on native failures and malformed mutation results", async () => {
    invoke.mockRejectedValueOnce(new Error("Disk full")).mockResolvedValueOnce({ books: [] })
      .mockResolvedValueOnce(snapshot({ ...book, favorite: "yes" as never }));
    await expect(setPrivateFavorite(book.id, true)).rejects.toThrow("Disk full");
    await expect(recordPrivateLibraryActivity(book.id)).rejects.toThrow("Invalid native");
    await expect(setPrivateFavorite(book.id, true)).rejects.toThrow("Invalid native");
    expect(get(privateLibrary)).toEqual(snapshot());
  });

  it("prevents a pending scan from overwriting a committed favorite", async () => {
    let finish!: (value: unknown) => void;
    invoke.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    const scan = refreshPrivateLibrary(true);
    invoke.mockResolvedValueOnce(snapshot({ ...book, favorite: true }));
    await setPrivateFavorite(book.id, true);
    finish(snapshot());
    await scan;
    expect(get(privateLibrary).books[0].favorite).toBe(true);
  });

  it("serializes canonical position writes with favorite snapshot application", async () => {
    let finish!: (value: unknown) => void;
    invoke.mockReturnValueOnce(new Promise(resolve => { finish = resolve; })).mockResolvedValueOnce(undefined);
    const favorite = setPrivateFavorite(book.id, true);
    const position = savePrivatePosition(book.id, { offsetMs: 42000 });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledOnce());
    expect(invoke).toHaveBeenCalledWith("reader_set_favorite", { bookId: book.id, favorite: true });
    finish(snapshot({ ...book, favorite: true }));
    await Promise.all([favorite, position]);
    expect(invoke).toHaveBeenNthCalledWith(2, "reader_save_position", { bookId: book.id, position: { offsetMs: 42000 } });
    expect(get(privateLibrary).books[0]).toMatchObject({ favorite: true, position: { offsetMs: 42000 } });
  });

  it("prevents a scan started during a position write from restoring old progress", async () => {
    let saved!: (value?: unknown) => void;
    let scanned!: (value: unknown) => void;
    invoke.mockReturnValueOnce(new Promise(resolve => { saved = resolve; }))
      .mockReturnValueOnce(new Promise(resolve => { scanned = resolve; }));
    const position = savePrivatePosition(book.id, { offsetMs: 42000 });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledOnce());
    const scan = refreshPrivateLibrary(true);
    saved();
    await position;
    scanned(snapshot());
    await scan;
    expect(get(privateLibrary).books[0].position).toEqual({ offsetMs: 42000 });
  });

  it("normalizes known YouTube formats and rejects unsafe URLs before native calls", async () => {
    for (const input of ["https://youtu.be/abcdefghijk?t=20", "https://www.youtube.com/embed/abcdefghijk",
      "http://m.youtube.com/shorts/abcdefghijk", "https://youtube.com/watch?v=abcdefghijk&x=y"]) {
      expect(normalizePrivateLibraryLink("youtube", input)).toBe("https://www.youtube.com/watch?v=abcdefghijk");
    }
    for (const input of ["javascript:alert(1)", "file:///media.mp4", "https://user:pass@example.com/a",
      "https://@example.com/a", "https://example.com/\nmedia", "https://example.com\\@evil.test/a"]) {
      await expect(addPrivateLibraryLink("Unsafe", "video", input)).rejects.toThrow();
    }
    for (const input of ["https://youtube.com.evil.test/watch?v=abcdefghijk", "https://youtu.be/short",
      "https://youtube.com/playlist?list=abcdefghijk"]) {
      await expect(addPrivateLibraryLink("Unsafe", "youtube", input)).rejects.toThrow();
    }
    expect(invoke).not.toHaveBeenCalled();
  });
});
