import { afterEach, describe, expect, it, vi } from "vitest";
import { mount, unmount, flushSync } from "svelte";
const ipc = vi.hoisted(() => ({ invoke: vi.fn(), graph: "/graph" }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("../lib/api", () => ({ getGraphInfo: async () => ({ path: ipc.graph }) }));
vi.mock("../lib/markdown", () => ({ assetBaseDirFor: () => "", hydrateAssetMedia: () => () => {} }));
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
  mounted = undefined; document.body.replaceChildren(); bookSelection.set(null); ipc.invoke.mockReset();
});

describe("original book notes panel", () => {
  it("captures source-pinned selection, saves explicit Markdown, and confirms removal", async () => {
    const book: BookInfo = { id: crypto.randomUUID(), pageId: "page", title: "Fixture", filePath: "books/file.epub",
      format: "epub", sourceSha256: "sha", readingLocation: null, indexingWarning: null };
    let notes: BookNote[] = [];
    ipc.invoke.mockImplementation(async (command, args) => {
      if (command === "book_open") return book;
      if (command === "book_notes_list") return notes;
      if (command === "book_note_save") {
        const note = { ...args, id: args.noteId, revision: "r1", status: "attached", notePageId: "note-page", filePath: "notes/note.md" };
        notes = [note]; return note;
      }
      if (command === "book_note_delete") { notes = []; return; }
      throw new Error(`Unexpected ${command}`);
    });
    mounted = mount(BookNotesPanel, { target: document.body, props: { pageId: "page", pageTitle: "Fixture" } });
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
    button("Remove").click(); flushSync();
    expect(document.querySelector('[role="alertdialog"]')).not.toBeNull();
    expect(ipc.invoke.mock.calls.some(c => c[0] === "book_note_delete")).toBe(false);
    button("Cancel").click(); flushSync();
    expect(document.querySelector('[role="alertdialog"]')).toBeNull();
    button("Remove").click(); flushSync(); button("Remove saved note").click();
    await vi.waitFor(() => expect(ipc.invoke.mock.calls.find(c => c[0] === "book_note_delete")?.[1]).toMatchObject({ expectedRevision: "r1" }));
    await vi.waitFor(() => expect(document.querySelectorAll(".book-note-card").length).toBe(0));
  });
});
