import { beforeEach, describe, expect, it, vi } from "vitest";
const ipc = vi.hoisted(() => ({ invoke: vi.fn(), getGraphInfo: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("./api", () => ({ getGraphInfo: ipc.getGraphInfo }));
import { BOOK_RENDERER_VERSION, type BookInfo, type BookNote } from "./books";
import { getBookNotesSource, newBookNoteDraft, editBookNote, saveBookNoteDraft, useBookSelection, bookDraftDirty, reviewBookNoteRevision, deleteBookNote } from "./bookNoteDrafts";

const makeBook = (): BookInfo => ({ id: crypto.randomUUID(), pageId: "page", title: "Test", format: "epub",
  filePath: "assets/test.epub", sourceSha256: "sha", readingLocation: null, indexingWarning: null });
beforeEach(() => { ipc.invoke.mockReset(); ipc.getGraphInfo.mockResolvedValue({ path: "/graph" }); });
describe("session original-book note drafts", () => {
  it("keeps drafts per graph/book across navigation without applying foreign selections", () => {
    const book = makeBook(), source = getBookNotesSource("/graph", book);
    const draft = newBookNoteDraft(source); draft.body = "Unsaved";
    expect(getBookNotesSource("/graph", book).drafts.get(draft.id)?.body).toBe("Unsaved");
    expect(getBookNotesSource("/other", book).drafts.size).toBe(0);
    useBookSelection(source, draft, { graphPath: "/other", pageId: "page", bookId: book.id,
      sourceSha256: "sha", quote: "wrong", locator: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: BOOK_RENDERER_VERSION } });
    expect(draft.locator).toBeNull(); expect(draft.error).toContain("another graph");
  });
  it("snapshots writes and preserves a newer body typed while the IPC is pending", async () => {
    const source = getBookNotesSource("/graph", makeBook()), draft = newBookNoteDraft(source);
    draft.body = "First";
    let finish!: (note: BookNote) => void;
    ipc.invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const saving = saveBookNoteDraft(source, draft);
    await vi.waitFor(() => expect(ipc.invoke).toHaveBeenCalled());
    draft.body = "Newer";
    const args = ipc.invoke.mock.calls[0][1];
    expect(args.graphPath).toBe("/graph"); expect(args.expectedRevision).toBeNull(); expect(args.noteId).toBe(draft.id);
    finish({ ...args, id: draft.id, revision: "r1", status: "attached" } as BookNote);
    await saving;
    expect(draft.body).toBe("Newer"); expect(bookDraftDirty(draft)).toBe(true);
    expect(draft.notice).toContain("Newer edits");
  });
  it("never overwrites a conflicting revision or crosses a graph switch", async () => {
    const source = getBookNotesSource("/graph", makeBook()), draft = newBookNoteDraft(source);
    draft.body = "Kept";
    ipc.invoke.mockRejectedValue(new Error("revision conflict"));
    await saveBookNoteDraft(source, draft);
    expect(draft.error).toContain("Refresh and review"); expect(draft.body).toBe("Kept");
    expect(draft.note).toBeNull();
    ipc.getGraphInfo.mockResolvedValue({ path: "/different" });
    ipc.invoke.mockClear();
    await saveBookNoteDraft(source, draft);
    expect(ipc.invoke).not.toHaveBeenCalled(); expect(draft.error).toContain("graph changed");
  });
  it("uses only an explicitly reviewed revision while keeping draft text", () => {
    const source = getBookNotesSource("/graph", makeBook()), draft = newBookNoteDraft(source);
    draft.body = "My changes";
    const saved = { id: draft.id, body: "External", quote: "", locator: null, sourceSha256: "sha", revision: "r2" } as BookNote;
    source.notes = [saved];
    reviewBookNoteRevision(source, draft, "r2");
    expect(draft.note?.revision).toBe("r2"); expect(draft.body).toBe("My changes");
    expect(bookDraftDirty(draft)).toBe(true);
  });
  it("keeps unsaved edits after explicitly removing their saved note", async () => {
    const source = getBookNotesSource("/graph", makeBook()), draft = newBookNoteDraft(source);
    const saved = { id: draft.id, body: "Saved", revision: "r1" } as BookNote;
    draft.note = saved; draft.body = "Unsaved changes"; source.notes = [saved];
    ipc.invoke.mockResolvedValue(undefined);
    await deleteBookNote(source, saved);
    expect(source.notes).toEqual([]); expect(draft.note).toBeNull();
    expect(draft.body).toBe("Unsaved changes"); expect(draft.id).not.toBe(saved.id);
  });
  it("allows orphan body edits without silently reattaching the saved anchor", async () => {
    const source = getBookNotesSource("/graph", makeBook());
    const note = { id: "orphan", bookId: source.book.id, body: "Saved", quote: "Old passage",
      sourceSha256: "old-hash", locator: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: BOOK_RENDERER_VERSION },
      revision: "r1", status: "orphaned" } as BookNote;
    const draft = editBookNote(source, note); draft.body = "Revised note text";
    ipc.invoke.mockResolvedValue({ ...note, body: draft.body, revision: "r2" });
    await saveBookNoteDraft(source, draft);
    expect(ipc.invoke).toHaveBeenCalledWith("book_note_save", expect.objectContaining({
      sourceSha256: "old-hash", quote: "Old passage", locator: note.locator, expectedRevision: "r1",
    }));
    expect(draft.note?.status).toBe("orphaned");
    expect(draft.error).toBe("");
  });
});
