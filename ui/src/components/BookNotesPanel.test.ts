import { afterEach, describe, expect, it, vi } from "vitest";
import { mount, unmount, flushSync } from "svelte";
const ipc = vi.hoisted(() => ({ invoke: vi.fn(), graph: "/graph",
  assetBase: vi.fn((path: string) => path.slice(0, path.lastIndexOf("/"))) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("../lib/api", () => ({ getGraphInfo: async () => ({ path: ipc.graph }) }));
vi.mock("../lib/markdown", () => ({ assetBaseDirFor: ipc.assetBase, hydrateAssetMedia: () => () => {} }));
vi.mock("../lib/readingNotes", () => ({
  readingNoteRelativePath: (_graph: string, path: string) => path,
  renderReadingNoteMarkdown: (body: string) => body.replace(/[<>&]/g, ""),
}));
import BookNotesPanel from "./BookNotesPanel.svelte";
import { bookSelection, BOOK_RENDERER_VERSION, type BookInfo, type BookNote } from "../lib/books";
let mounted: ReturnType<typeof mount> | undefined;
const button = (label: string) => [...document.querySelectorAll("button")].find(b => b.textContent?.trim() === label)!;
afterEach(async () => {
  if (mounted) await unmount(mounted);
  mounted = undefined; document.body.replaceChildren(); bookSelection.set(null); ipc.invoke.mockReset(); ipc.assetBase.mockClear();
});

describe("original book notes panel", () => {
  it("captures source-pinned selection, saves an adjacent annotation, and confirms removal", async () => {
    const navigate = vi.fn();
    const book: BookInfo = { id: crypto.randomUUID(), pageId: "page", title: "Fixture", filePath: "books/file.epub",
      format: "epub", sourceSha256: "sha", readingLocation: null, indexingWarning: null };
    let notes: BookNote[] = [];
    ipc.invoke.mockImplementation(async (command, args) => {
      if (command === "book_notes_context") return book;
      if (command === "book_notes_list") return notes;
      if (command === "book_note_save") {
        const note = { ...args, id: args.noteId, revision: "r1", conflicts: [], status: "attached", notePageId: "note-page", filePath: "books/file.jsonld" };
        notes = [note]; return note;
      }
      if (command === "book_note_delete") { notes = []; return; }
      throw new Error(`Unexpected ${command}`);
    });
    mounted = mount(BookNotesPanel, { target: document.body, props: { pageId: "page", pageTitle: "Fixture", onNavigate: navigate } });
    await vi.waitFor(() => expect(document.querySelector("textarea")).not.toBeNull());
    expect(document.querySelector(".book-notes-panel")?.getAttribute("data-help-context")).toBe("books");
    bookSelection.set({ graphPath: "/graph", bookId: book.id, pageId: "page", sourceSha256: "sha", quote: "Original quote",
      locator: { kind: "epub", cfi: "epubcfi(/6/2!/4/2:0)", rendererVersion: BOOK_RENDERER_VERSION } });
    flushSync();
    button("Use selection").click(); flushSync();
    expect(document.querySelector("blockquote")?.textContent).toBe("Original quote");
    const textarea = document.querySelector("textarea")!;
    textarea.value = "My note"; textarea.dispatchEvent(new Event("input", { bubbles: true })); flushSync();
    button("Save note").click();
    await vi.waitFor(() => expect(document.querySelectorAll(".book-note-card").length).toBe(1));
    const write = ipc.invoke.mock.calls.find(c => c[0] === "book_note_save");
    expect(write?.[1]).toMatchObject({ graphPath: "/graph", bookId: book.id, quote: "Original quote", body: "My note", expectedRevision: null });
    expect(ipc.assetBase).toHaveBeenCalledWith("books/file.jsonld");
    expect(button("Open note")).toBeDefined();
    button("Open note").click(); flushSync();
    expect(navigate).toHaveBeenCalledWith({ id: "note-page" });
    expect(document.querySelector("textarea")?.value).toBe("My note");
    button("Remove").click(); flushSync();
    expect(document.querySelector('[role="alertdialog"]')).not.toBeNull();
    expect(ipc.invoke.mock.calls.some(c => c[0] === "book_note_delete")).toBe(false);
    button("Cancel").click(); flushSync();
    expect(document.querySelector('[role="alertdialog"]')).toBeNull();
    button("Remove").click(); flushSync(); button("Remove saved note").click();
    await vi.waitFor(() => expect(ipc.invoke.mock.calls.find(c => c[0] === "book_note_delete")?.[1]).toMatchObject({ expectedRevision: "r1" }));
    await vi.waitFor(() => expect(document.querySelectorAll(".book-note-card").length).toBe(0));
  });
  it("keeps legacy Markdown notes on their existing editor route and media base", async () => {
    const navigate = vi.fn();
    const book: BookInfo = { id: crypto.randomUUID(), pageId: "page", title: "Fixture", filePath: "books/file.epub",
      format: "epub", sourceSha256: "sha", readingLocation: null, indexingWarning: null };
    ipc.invoke.mockImplementation(async command => {
      if (command === "book_notes_context") return book;
      if (command === "book_notes_list") return [{ id: "legacy", bookId: book.id, notePageId: "legacy-page",
        filePath: "pages/Reading Notes/Books/legacy.md", body: "Legacy", quote: "", locator: null,
        sourceSha256: "sha", conflicts: [], revision: "old", status: "attached" }];
    });
    mounted = mount(BookNotesPanel, { target: document.body, props: { pageId: "page", pageTitle: "Fixture", onNavigate: navigate } });
    await vi.waitFor(() => expect(document.querySelectorAll(".book-note-card")).toHaveLength(1));
    button("Open note").click();
    expect(navigate).toHaveBeenCalledWith({ id: "legacy-page" });
    expect(ipc.assetBase).toHaveBeenCalledWith("pages/Reading Notes/Books/legacy.md");
  });

  function conflictFixture(deleted = false) {
    const book: BookInfo = { id: crypto.randomUUID(), pageId: "page", title: "Fixture", filePath: "books/file.epub",
      format: "epub", sourceSha256: "sha", readingLocation: null, indexingWarning: null };
    const note: BookNote = { id: crypto.randomUUID(), bookId: book.id, notePageId: "note-page", filePath: "books/file.jsonld",
      body: "", quote: "", locator: null, sourceSha256: "", revision: "aggregate-1",
      createdAt: "", updatedAt: "", status: "conflicted", conflicts: [
        { revision: "first", body: "First candidate", quote: "First quote", locator: null, sourceSha256: "old-sha", updatedAt: "Monday", deleted: false },
        { revision: "second", body: "Second candidate", quote: "Second quote",
          locator: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: BOOK_RENDERER_VERSION },
          sourceSha256: "sha", updatedAt: "Tuesday", deleted },
      ] };
    let notes = [note], fail = false;
    ipc.invoke.mockImplementation(async (command, args) => {
      if (command === "book_notes_context") return book;
      if (command === "book_notes_list") return notes;
      if (command === "book_note_resolve") {
        if (fail) {
          notes = [{ ...note, revision: "aggregate-2", conflicts: [...note.conflicts, { ...note.conflicts[0], revision: "third", body: "New remote edit" }] }];
          throw new Error("revision conflict");
        }
        notes = args.delete ? [] : [{ ...note, ...args, revision: "merged", conflicts: [] }];
        return;
      }
      throw new Error(`Unexpected ${command}`);
    });
    return { book, note, stale: () => { fail = true; } };
  }
  async function openFixture() {
    mounted = mount(BookNotesPanel, { target: document.body, props: { pageId: "page", pageTitle: "Fixture" } });
    await vi.waitFor(() => expect(document.querySelectorAll(".conflict-candidate")).toHaveLength(2));
  }
  it("shows distinguishable complete candidates and resolves only from the explicit merged-note button", async () => {
    conflictFixture(); await openFixture();
    const candidates = [...document.querySelectorAll(".conflict-candidate")];
    expect(candidates[0].textContent).toContain("First candidate");
    expect(candidates[0].textContent).toContain("old-sha");
    expect(candidates[1].textContent).toContain("Second quote");
    expect(candidates[1].textContent).toContain("epubcfi(/6/2)");
    button("Edit merge draft").click(); flushSync();
    expect(document.querySelector("textarea")?.value).toBe("");
    expect(button("Resolve with merged note").disabled).toBe(true);
    expect(button("Save note")).toBeUndefined();
    button("Use candidate 1 text and attachment").click(); flushSync();
    const textarea = document.querySelector("textarea")!;
    textarea.value = "First and second combined"; textarea.dispatchEvent(new Event("input", { bubbles: true })); flushSync();
    button("Use candidate 2 attachment only").click(); flushSync();
    expect(textarea.value).toBe("First and second combined");
    expect(ipc.invoke.mock.calls.some(c => ["book_note_save", "book_note_resolve"].includes(c[0]))).toBe(false);
    button("Resolve with merged note").click();
    await vi.waitFor(() => expect(document.querySelectorAll(".conflict-candidate")).toHaveLength(0));
    expect(ipc.invoke).toHaveBeenCalledWith("book_note_resolve", expect.objectContaining({
      body: "First and second combined", quote: "Second quote", expectedRevision: "aggregate-1", delete: false,
    }));
  });
  it("requires a confirmation to resolve an edit/deletion conflict", async () => {
    conflictFixture(true); await openFixture();
    expect(document.querySelectorAll(".conflict-candidate")[1].textContent).toContain("Deleted");
    button("Resolve as deleted…").click(); flushSync();
    expect(document.querySelector('[aria-label="Resolve book note as deleted"]')).not.toBeNull();
    expect(ipc.invoke.mock.calls.some(c => c[0] === "book_note_resolve")).toBe(false);
    button("Cancel").click(); flushSync();
    expect(document.querySelectorAll(".conflict-candidate")).toHaveLength(2);
    button("Resolve as deleted…").click(); flushSync();
    button("Confirm deletion resolution").click();
    await vi.waitFor(() => expect(document.querySelectorAll(".conflict-candidate")).toHaveLength(0));
    expect(ipc.invoke).toHaveBeenCalledWith("book_note_resolve", expect.objectContaining({
      delete: true, expectedRevision: "aggregate-1", sourceSha256: "sha", locator: null, quote: "", body: "",
    }));
    expect(ipc.invoke.mock.calls.some(c => c[0] === "book_note_delete")).toBe(false);
  });
  it("keeps the merged draft and shows refreshed candidates after a stale resolution", async () => {
    const fixture = conflictFixture(); fixture.stale(); await openFixture();
    button("Use candidate 1 text and attachment").click(); flushSync();
    const textarea = document.querySelector("textarea")!;
    textarea.value = "Do not lose my merge"; textarea.dispatchEvent(new Event("input", { bubbles: true })); flushSync();
    button("Resolve with merged note").click();
    await vi.waitFor(() => expect(document.querySelectorAll(".conflict-candidate")).toHaveLength(3));
    expect(textarea.value).toBe("Do not lose my merge");
    expect(button("Resolve with merged note").disabled).toBe(true);
    expect(document.body.textContent).toContain("Could not resolve");
    button("Use candidate 3 attachment only").click(); flushSync();
    expect(textarea.value).toBe("Do not lose my merge");
    expect(button("Resolve with merged note").disabled).toBe(false);
  });
  it("opens orphan conflicts without a reader and requires a chosen anchor to delete", async () => {
    const fixture = conflictFixture(true);
    fixture.book.sourceAvailable = false; fixture.book.sourceSha256 = "";
    await openFixture();
    expect(ipc.invoke.mock.calls.some(c => c[0] === "book_open")).toBe(false);
    expect(document.body.textContent).toContain("Original unavailable");
    expect(button("New note").disabled).toBe(true);
    button("Resolve as deleted…").click(); flushSync();
    expect(button("Whole-book note").disabled).toBe(true);
    expect(button("Confirm deletion resolution").disabled).toBe(true);
    button("Cancel").click(); flushSync();
    button("Use candidate 2 attachment only").click(); flushSync();
    button("Resolve as deleted…").click(); flushSync();
    expect(button("Confirm deletion resolution").disabled).toBe(false);
    button("Confirm deletion resolution").click();
    await vi.waitFor(() => expect(ipc.invoke).toHaveBeenCalledWith("book_note_resolve", expect.objectContaining({
      sourceSha256: "sha", quote: "Second quote", locator: fixture.note.conflicts[1].locator, delete: true,
    })));
  });
});
