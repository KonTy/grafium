import { describe, expect, it } from "vitest";
import { BOOK_RENDERER_VERSION, compatibleBookLocation, isBookLocation, isOriginalBookPage, selectionForBook, jumpToBookNote, isBookAnnotationPage, bookAnnotationTarget, type BookInfo, type BookNote } from "./books";
import { BOOK_FRAME_SANDBOX, readerFrameURL, readReaderMessage, sanitizeBookDocument } from "./bookReaderSecurity";
import { assetBaseDirFor } from "./markdown";
import { readingNoteRelativePath, renderReadingNoteMarkdown } from "./readingNotes";
import type { Page } from "./api";

const book: BookInfo = { id: "book", pageId: "page", title: "Book", format: "epub", filePath: "assets/book.epub",
  sourceSha256: "hash", readingLocation: null, indexingWarning: null };
const locator = { kind: "epub" as const, cfi: "epubcfi(/6/2!/4/2:0)", rendererVersion: BOOK_RENDERER_VERSION };
describe("original book boundaries", () => {
  it("only routes complete original-book metadata", () => {
    const page = { properties: { "book-id": "book", "book-format": "epub", "book-source-sha256": "hash", "book-source": "assets/book.epub" } } as unknown as Page;
    expect(isOriginalBookPage(page)).toBe(true);
    expect(isOriginalBookPage({ ...page, properties: { "book-id": "book" } })).toBe(false);
  });
  it("routes only explicit virtual annotation pages and rejects malformed references without treating them as Markdown", () => {
    const page = { properties: { "book-annotation": true, "book-note": JSON.stringify({ id: "note", bookId: "book" }) } } as unknown as Page;
    expect(isBookAnnotationPage(page)).toBe(true);
    expect(bookAnnotationTarget(page)).toEqual({ bookId: "book", noteId: "note" });
    expect(bookAnnotationTarget({ ...page, properties: { "book-annotation": true,
      "book-note-id": "direct-note", "book-note-book-page-id": "direct-book" } }))
      .toEqual({ bookId: "direct-book", noteId: "direct-note" });
    expect(bookAnnotationTarget({ ...page, properties: { "book-note": page.properties["book-note"] } })).toBeNull();
    for (const metadata of ["invalid", "null", "{}", '{"id":"note","bookId":false}']) {
      const broken = { ...page, properties: { ...page.properties, "book-note": metadata } };
      expect(isBookAnnotationPage(broken)).toBe(true);
      expect(bookAnnotationTarget(broken)).toBeNull();
    }
  });
  it("guards finite normalized unrotated PDF rectangles and one-based pages", () => {
    expect(isBookLocation({ kind: "pdf", page: 1, rects: [{ x: .1, y: .2, width: .3, height: .4 }] })).toBe(true);
    for (const page of [0, -1, 1.2, Infinity]) expect(isBookLocation({ kind: "pdf", page })).toBe(false);
    for (const x of [NaN, -.1, Infinity, 1.1]) expect(isBookLocation({ kind: "pdf", page: 1, rects: [{ x, y: 0, width: .1, height: .2 }] })).toBe(false);
    expect(isBookLocation({ kind: "pdf", page: 1, rects: [{ x: .9, y: 0, width: .2, height: .2 }] })).toBe(false);
  });
  it("never applies a selection to another graph, source, page, or renderer", () => {
    const selection = { graphPath: "/a", bookId: book.id, pageId: book.pageId, sourceSha256: "hash", quote: "passage", locator };
    expect(selectionForBook(selection, "/a", book)).toEqual(selection);
    expect(selectionForBook(selection, "/a", { ...book, sourceAvailable: false })).toBeNull();
    expect(selectionForBook(selection, "/b", book)).toBeNull();
    for (const key of ["bookId", "pageId", "sourceSha256"]) expect(selectionForBook({ ...selection, [key]: "other" }, "/a", book)).toBeNull();
    expect(compatibleBookLocation(book, { ...locator, rendererVersion: "old" })).toBe(false);
    expect(compatibleBookLocation(book, { kind: "pdf", page: 1 })).toBe(false);
  });
  it("does not choose one conflicting attachment for a passage jump", () => {
    const candidate = { body: "Note", quote: "Quote", locator, sourceSha256: "hash", updatedAt: "", deleted: false };
    const note: BookNote = { id: "note", notePageId: "note-page", filePath: "books/book.jsonld",
      body: "", quote: "", revision: "heads", createdAt: "", updatedAt: "",
      bookId: book.id, sourceSha256: "hash", status: "attached", locator,
      conflicts: [{ ...candidate, revision: "first" }, { ...candidate, revision: "second" }] };
    expect(() => jumpToBookNote("/graph", book, note)).toThrow("passage is unavailable");
  });
  it("previews adjacent annotation media from the book folder without changing legacy Markdown paths", () => {
    for (const [path, expected] of [
      ["/graph/books/1.jsonld", "books"],
      ["/graph/pages/Reading Notes/Books/old.md", "pages/Reading%20Notes/Books"],
    ]) {
      const base = assetBaseDirFor(readingNoteRelativePath("/graph", path));
      const html = renderReadingNoteMarkdown("![Figure](assets/figure.png)", base);
      expect(html).toContain(`grafium-asset://localhost/${expected}/assets/figure.png`);
      expect(html).not.toContain("/graph/");
    }
  });
  it("removes active markup and resource links, preserving authored prose and inline layout", () => {
    const html = sanitizeBookDocument(`<html><head><base href="https://evil.test/"><meta http-equiv="refresh" content="0;url=https://evil.test/">
      <link rel="stylesheet" href="blob:null/local"><link rel="preload" href="https://evil.test/x"></head><body>
      <script>parent.pwned=true</script><p id="passage" style="font-style:italic" onclick="evil()">Words <em>kept</em></p>
      <img src="https://evil.test/a" onerror="evil()"><img src="blob:null/image"><iframe srcdoc="evil"></iframe>
      <a href="javascript:evil()">No</a><a href="//evil.test">No</a><a href="chapter.xhtml#p">Yes</a>
      <svg><script>evil()</script><foreignObject><iframe src="x"></iframe></foreignObject></svg></body></html>`);
    const doc = new DOMParser().parseFromString(html, "text/html");
    expect(doc.querySelector("script,iframe,base,form,foreignObject")).toBeNull();
    expect(doc.querySelector("[onclick],[onerror],[src^='https:'],[href^='https:']")).toBeNull();
    expect(doc.querySelector("p")?.id).toBe("passage");
    expect(doc.querySelector("p")?.getAttribute("style")).toBe("font-style:italic");
    expect(doc.querySelector("meta")?.content).toContain("script-src 'none'");
    expect(doc.querySelector("a[href]")?.getAttribute("href")).toBe("chapter.xhtml#p");
    expect(doc.querySelector("img[src]")?.getAttribute("src")).toBe("blob:null/image");
  });
  it("uses a data origin, never app-origin srcdoc, and does not grant navigation", () => {
    const url = readerFrameURL("nonce-123");
    expect(url.startsWith("data:text/html;")).toBe(true);
    expect(decodeURIComponent(url)).toContain("connect-src blob:");
    expect(BOOK_FRAME_SANDBOX).not.toMatch(/allow-(top-navigation|popups|forms|downloads)/);
    expect(() => readerFrameURL("';script-src *")).toThrow();
  });
  it("requires exact frame, opaque origin, token, and bounded message schema", () => {
    const source = window;
    const data = { channel: "grafium-book", token: "secret", type: "selection", quote: "Words", location: locator };
    const event = { data, source, origin: "null" } as unknown as MessageEvent;
    expect(readReaderMessage(event, source, "secret")?.type).toBe("selection");
    expect(readReaderMessage({ ...event, source: {} } as MessageEvent, source, "secret")).toBeNull();
    expect(readReaderMessage({ ...event, origin: "https://evil.test" } as MessageEvent, source, "secret")).toBeNull();
    expect(readReaderMessage(event, source, "wrong")).toBeNull();
    expect(readReaderMessage({ ...event, data: { ...data, location: { kind: "pdf", page: -1 } } } as MessageEvent, source, "secret")).toBeNull();
    expect(readReaderMessage({ ...event, data: { ...data, type: "invoke", command: "shell" } } as MessageEvent, source, "secret")).toBeNull();
  });
});
