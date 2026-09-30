import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("../lib/api", () => ({ getGraphInfo: async () => ({ path: "/graph" }) }));
vi.mock("../lib/markdown", () => ({ assetBaseDirFor: () => "books", hydrateAssetMedia: () => () => {} }));
vi.mock("../lib/readingNotes", () => ({
  readingNoteRelativePath: (_graph: string, path: string) => path,
  renderReadingNoteMarkdown: (body: string) => body.replace(/[<>&]/g, ""),
}));
import BookAnnotationPage from "./BookAnnotationPage.svelte";
import type { Page } from "../lib/api";
import type { BookInfo, BookNote } from "../lib/books";
let mounted: ReturnType<typeof mount> | undefined;
afterEach(async () => {
  if (mounted) await unmount(mounted);
  mounted = undefined; document.body.replaceChildren(); ipc.invoke.mockReset();
});

describe("virtual annotation editor routing", () => {
  it.each([false, true])("opens the selected annotation safely (conflicted: %s)", async conflicted => {
    const bookId = crypto.randomUUID(), noteId = crypto.randomUUID(), navigate = vi.fn();
    const book: BookInfo = { id: bookId, pageId: bookId, title: "Original", format: "epub",
      filePath: `books/${bookId}/original.epub`, sourceSha256: "sha", readingLocation: null, indexingWarning: null };
    const note: BookNote = { id: noteId, bookId, notePageId: noteId, filePath: `books/${bookId}/original.jsonld`,
      body: conflicted ? "" : "Saved annotation", quote: "", locator: null, sourceSha256: conflicted ? "" : "sha",
      revision: "heads", createdAt: "", updatedAt: "", status: conflicted ? "conflicted" : "attached",
      conflicts: conflicted ? ["First", "Second"].map(body => ({ revision: body, body, quote: "", locator: null,
        sourceSha256: "sha", updatedAt: "", deleted: false })) : [] };
    ipc.invoke.mockImplementation(async command => command === "book_notes_context" ? book : command === "book_notes_list" ? [note] : undefined);
    const page = { id: noteId, title: "Annotation", properties: { "book-annotation": true,
      "book-note": JSON.stringify({ id: noteId, bookId }) } } as unknown as Page;
    mounted = mount(BookAnnotationPage, { target: document.body, props: { page, onNavigate: navigate } });
    await vi.waitFor(() => expect(document.querySelector("textarea")).not.toBeNull());
    expect(document.querySelector("textarea")?.value).toBe(conflicted ? "" : "Saved annotation");
    expect(ipc.invoke).toHaveBeenCalledWith("book_notes_context", { graphPath: "/graph", bookId });
    expect(document.querySelector('[aria-label="Book annotation editor"]')?.getAttribute("data-help-context")).toBe("books");
    expect(ipc.invoke.mock.calls.every(([command]) => ["book_notes_context", "book_notes_list"].includes(command))).toBe(true);
    expect(document.body.textContent).not.toContain("older source");
    document.querySelector<HTMLButtonElement>("button.return")!.click(); flushSync();
    expect(navigate).toHaveBeenCalledWith({ id: bookId });
  });
  it("does not fall back to a file editor when the virtual page reference is malformed", () => {
    const page = { id: "note", title: "Broken reference", properties: { "book-annotation": true, "book-note": "bad json" } } as unknown as Page;
    mounted = mount(BookAnnotationPage, { target: document.body, props: { page, onNavigate: vi.fn() } });
    flushSync();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("has not been changed");
    expect(document.querySelector("textarea")).toBeNull();
    expect(ipc.invoke).not.toHaveBeenCalled();
  });
});
