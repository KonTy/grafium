import { describe, expect, it, vi } from "vitest";
import { libraryBookmarkJournalSnippet, libraryLink, parseLibraryLink, routeLibraryLink } from "./libraryLinks";
import { studyClockItem, studyDisplayProgress } from "./studyLibrary";
import { libraryStudyCatalog } from "./studyCatalog";
import type { ReaderBook, ReaderBookmark } from "./privateReader";
import type { StudyItem } from "./studies";
import { extractMarkdownReferences, renderBlock } from "./markdown";

const bookId = "12345678-1234-4234-8234-123456789abc";
const bookmarkId = "87654321-4321-4321-8321-cba987654321";
const book: ReaderBook = {
  id: bookId, title: "My [[book]]", kind: "audio", available: true,
  tracks: [{ id: "track", title: "Chapter 1", relativePath: "private/track.mp3" }],
  position: { trackId: "track", offsetMs: 12000 }, bookmarks: [],
  progress: { position: 12, total: 100, anchor: "", label: "12%" },
};
const bookmark: ReaderBookmark = {
  id: bookmarkId, bookId, position: { trackId: "track", offsetMs: 12000 }, createdAt: 0, note: "My deliberate note",
};
const item: StudyItem = {
  id: "plan", title: book.title, kind: "library", source: bookId, topic: "General",
  createdAt: "", updatedAt: "", progress: { position: 90, total: 100, anchor: "", label: "Stale copy" },
};

describe("Library references and study adapter", () => {
  it("routes journal clicks to the exact bookmark before graph-link handlers run", () => {
    const open = vi.fn(), invalid = vi.fn(), graphClick = vi.fn();
    const root = document.createElement("div");
    root.innerHTML = renderBlock(libraryBookmarkJournalSnippet(book, bookmark));
    root.addEventListener("click", event => routeLibraryLink(event, open, invalid), true);
    const anchor = root.querySelector("a")!;
    anchor.addEventListener("click", graphClick);
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    anchor.dispatchEvent(event);
    expect(open).toHaveBeenCalledWith(bookId, bookmarkId);
    expect(graphClick).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
    anchor.setAttribute("href", "#grafium-library/invalid");
    anchor.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(invalid).toHaveBeenCalledOnce();
    anchor.setAttribute("href", "#ordinary-heading");
    const external = new MouseEvent("click", { bubbles: true, cancelable: true });
    anchor.dispatchEvent(external);
    expect(graphClick).toHaveBeenCalledOnce();
    expect(external.defaultPrevented).toBe(false);
  });
  it("round-trips only stable item and bookmark identifiers", () => {
    expect(parseLibraryLink(libraryLink(bookId, bookmarkId))).toEqual({ bookId, bookmarkId });
    expect(parseLibraryLink(libraryLink(bookId))).toEqual({ bookId });
    for (const href of ["#grafium-library/../../private", "#grafium-library/file:///book.mp3",
      `#grafium-library/${bookId}?url=https://example.com`, `https://example.com/${libraryLink(bookId)}`])
      expect(parseLibraryLink(href)).toBeNull();
    expect(() => libraryLink("/private/book")).toThrow();
  });
  it("writes deliberate journal notes without filesystem paths or automatic wiki pages", () => {
    const snippet = libraryBookmarkJournalSnippet(book, bookmark);
    expect(snippet).toContain(libraryLink(bookId, bookmarkId));
    expect(snippet).toContain("My deliberate note");
    expect(snippet).not.toContain("private/track.mp3");
    expect(snippet).not.toContain("[[book]]");
    const html = renderBlock(snippet);
    expect(html).toContain(`href="${libraryLink(bookId, bookmarkId)}"`);
    expect(html).not.toContain('data-page=');
    expect(extractMarkdownReferences(snippet)).toEqual({ pages: [], tags: [] });
    expect(() => libraryBookmarkJournalSnippet(book, { ...bookmark, bookId: bookmarkId })).toThrow();
  });
  it("keeps Library paths out of the plan catalog and reads the owner's progress", () => {
    const [candidate] = libraryStudyCatalog([book]);
    expect(candidate).toMatchObject({ source: bookId, kind: "library", title: book.title });
    expect(JSON.stringify(candidate)).not.toContain("private/track.mp3");
    expect(studyDisplayProgress(item, [book])).toEqual(book.progress);
    expect(studyDisplayProgress(item, []).label).toContain("unavailable");
    expect(studyClockItem(item, [book]).kind).toBe("audio");
    expect(studyClockItem(item, [{ ...book, kind: "epub" }]).kind).toBe("book");
    expect(() => studyClockItem(item, [])).toThrow("unavailable");
  });
});
