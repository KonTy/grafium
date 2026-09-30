import { beforeEach, describe, expect, it, vi } from "vitest";
const ipc = vi.hoisted(() => ({ invoke: vi.fn(), getGraphInfo: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("./api", () => ({ getGraphInfo: ipc.getGraphInfo }));
import { BOOK_RENDERER_VERSION, type BookInfo, type BookNote } from "./books";
import { getBookNotesSource, newBookNoteDraft, editBookNote, saveBookNoteDraft, useBookSelection, bookDraftDirty, reviewBookNoteRevision, deleteBookNote, chooseBookNoteCandidate, resolveBookNoteDraft, clearBookPassage } from "./bookNoteDrafts";

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

  describe("manual annotation conflict resolution", () => {
    function conflictSource() {
      const source = getBookNotesSource("/graph", makeBook());
      const note: BookNote = { id: crypto.randomUUID(), bookId: source.book.id, notePageId: "virtual-note",
        filePath: "books/book.jsonld", body: "Must not implicitly select", quote: "old", locator: null,
        sourceSha256: "sha", revision: "heads-1", createdAt: "", updatedAt: "", status: "attached",
        conflicts: [
          { revision: "a", body: "First edit", quote: "Passage A", locator: null, sourceSha256: "old-sha", updatedAt: "today", deleted: false },
          { revision: "b", body: "Second edit", quote: "Passage B", locator: null, sourceSha256: "sha", updatedAt: "today", deleted: false },
        ] };
      source.notes = [note];
      return { source, note, draft: editBookNote(source, note) };
    }
    it("never preselects a conflicting body or resolves through ordinary save/delete", async () => {
      const { source, note, draft } = conflictSource();
      expect(draft.body).toBe(""); expect(draft.attachmentChosen).toBe(false);
      draft.body = "Manually combined";
      await saveBookNoteDraft(source, draft);
      await expect(deleteBookNote(source, note)).rejects.toThrow("explicit resolution");
      await resolveBookNoteDraft(source, draft);
      expect(ipc.invoke).not.toHaveBeenCalled();
      expect(draft.error).toContain("explicitly choose");
    });
    it("resolves only an explicitly chosen attachment and manually merged body with the aggregate fence", async () => {
      const { source, note, draft } = conflictSource();
      chooseBookNoteCandidate(source, note, note.conflicts[0]);
      draft.body = "Both edits combined";
      expect(ipc.invoke).not.toHaveBeenCalled();
      ipc.invoke.mockImplementation(async command => command === "book_notes_list"
        ? [{ ...note, body: draft.body, conflicts: [], revision: "resolved" }] : undefined);
      await resolveBookNoteDraft(source, draft);
      expect(ipc.invoke).toHaveBeenCalledWith("book_note_resolve", expect.objectContaining({
        expectedRevision: "heads-1", body: "Both edits combined", quote: "Passage A", sourceSha256: "old-sha", delete: false,
      }));
      expect(draft.note?.conflicts).toEqual([]); expect(draft.error).toBe("");
    });
    it("keeps a merged draft and refreshes candidates after a stale resolution", async () => {
      const { source, note, draft } = conflictSource();
      chooseBookNoteCandidate(source, note, note.conflicts[1]);
      draft.body = "My careful merge";
      const latest = { ...note, revision: "heads-2", conflicts: [...note.conflicts, { ...note.conflicts[0], revision: "c", body: "Third edit" }] };
      ipc.invoke.mockImplementation(async command => {
        if (command === "book_note_resolve") throw new Error("revision conflict");
        if (command === "book_notes_list") return [latest];
      });
      await resolveBookNoteDraft(source, draft);
      expect(draft.body).toBe("My careful merge"); expect(draft.quote).toBe("Passage B");
      expect(source.notes[0].conflicts).toHaveLength(3); expect(draft.attachmentChosen).toBe(false);
      expect(draft.note?.revision).toBe("heads-1");
      const count = ipc.invoke.mock.calls.length;
      await resolveBookNoteDraft(source, draft);
      expect(ipc.invoke).toHaveBeenCalledTimes(count);
      chooseBookNoteCandidate(source, latest, latest.conflicts[2], false);
      expect(draft.body).toBe("My careful merge"); expect(draft.note?.revision).toBe("heads-2");
    });
    it("explicitly resolves deletion and retains composer text as a new draft", async () => {
      const { source, note, draft } = conflictSource();
      note.sourceSha256 = ""; draft.sourceSha256 = "";
      note.conflicts[1].deleted = true;
      draft.body = "Preserve these unsaved ideas";
      ipc.invoke.mockImplementation(async command => command === "book_notes_list" ? [] : undefined);
      await resolveBookNoteDraft(source, draft, true);
      expect(ipc.invoke).toHaveBeenCalledWith("book_note_resolve", expect.objectContaining({
        delete: true, expectedRevision: "heads-1", sourceSha256: source.book.sourceSha256, locator: null, quote: "", body: "",
      }));
      expect(ipc.invoke.mock.calls.some(([command]) => command === "book_note_delete")).toBe(false);
      expect(draft.body).toBe("Preserve these unsaved ideas"); expect(draft.note).toBeNull(); expect(draft.id).not.toBe(note.id);
      expect(draft.sourceSha256).toBe(source.book.sourceSha256);
    });
    it("does not retry a successful resolution as a new note if the list reload fails", async () => {
      const { source, note, draft } = conflictSource();
      chooseBookNoteCandidate(source, note, note.conflicts[0]);
      ipc.invoke.mockImplementation(async command => {
        if (command === "book_notes_list") throw new Error("read unavailable");
      });
      await resolveBookNoteDraft(source, draft);
      expect(draft.body).toBe("First edit"); expect(draft.note?.id).toBe(note.id);
      expect(source.error).toContain("read unavailable");
      await saveBookNoteDraft(source, draft);
      expect(ipc.invoke.mock.calls.some(([command]) => command === "book_note_save")).toBe(false);
    });
    it("allows explicit whole-book reattachment without choosing a candidate's text", () => {
      const { source, draft } = conflictSource();
      draft.body = "Independent merge";
      clearBookPassage(source, draft);
      expect(draft.attachmentChosen).toBe(true); expect(draft.locator).toBeNull();
      expect(draft.sourceSha256).toBe("sha"); expect(draft.body).toBe("Independent merge");
    });
    it("keeps orphan conflicts readable and requires an explicit saved anchor even for deletion", async () => {
      const { source, note, draft } = conflictSource();
      source.book.sourceAvailable = false; source.book.sourceSha256 = "";
      note.sourceSha256 = ""; draft.sourceSha256 = "";
      note.conflicts[1].deleted = true;
      clearBookPassage(source, draft);
      expect(draft.attachmentChosen).toBe(false);
      await resolveBookNoteDraft(source, draft, true);
      expect(ipc.invoke).not.toHaveBeenCalled();
      expect(draft.error).toContain("Explicitly choose");
      chooseBookNoteCandidate(source, note, note.conflicts[1], false);
      reviewBookNoteRevision(source, draft, note.revision);
      expect(draft.attachmentChosen).toBe(true);
      ipc.invoke.mockImplementation(async command => command === "book_notes_list" ? [] : undefined);
      await resolveBookNoteDraft(source, draft, true);
      expect(ipc.invoke).toHaveBeenCalledWith("book_note_resolve", expect.objectContaining({
        delete: true, expectedRevision: "heads-1", sourceSha256: "sha", quote: "Passage B", locator: null,
      }));
      ipc.invoke.mockClear();
      draft.body = "New orphan draft";
      await saveBookNoteDraft(source, draft);
      expect(ipc.invoke).not.toHaveBeenCalled();
      expect(draft.error).toContain("new notes require");
    });
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
    const note: BookNote = { id: "orphan", bookId: source.book.id, body: "Saved", quote: "Old passage",
      notePageId: "legacy-note", createdAt: "", updatedAt: "",
      filePath: "pages/Reading Notes/Books/old.md", conflicts: [],
      sourceSha256: "old-hash", locator: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: BOOK_RENDERER_VERSION },
      revision: "r1", status: "orphaned" };
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
