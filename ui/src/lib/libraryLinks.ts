import type { ReaderBook, ReaderBookmark } from "./privateReader";
import { bookmarkLabel } from "./privateReader";

const UUID = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
const LINK = new RegExp(`^#grafium-library/(${UUID})(?:\\?bookmark=(${UUID}))?$`, "i");

export function parseLibraryLink(href: string): { bookId: string; bookmarkId?: string } | null {
  const match = LINK.exec(href);
  return match ? { bookId: match[1].toLowerCase(), ...(match[2] ? { bookmarkId: match[2].toLowerCase() } : {}) } : null;
}

export function libraryLink(bookId: string, bookmarkId?: string): string {
  const href = `#grafium-library/${bookId}${bookmarkId ? `?bookmark=${bookmarkId}` : ""}`;
  if (!parseLibraryLink(href)) throw new Error("Invalid Library item or bookmark ID.");
  return href;
}

export function routeLibraryLink(
  event: MouseEvent,
  open: (bookId: string, bookmarkId?: string) => void,
  invalid: () => void,
): void {
  const anchor = event.target instanceof Element ? event.target.closest("a[href]") : null;
  const href = anchor?.getAttribute("href");
  if (!href?.startsWith("#grafium-library/")) return;
  event.preventDefault();
  event.stopPropagation();
  const target = parseLibraryLink(href);
  if (target) open(target.bookId, target.bookmarkId);
  else invalid();
}

export function libraryBookmarkJournalSnippet(book: ReaderBook, bookmark: ReaderBookmark): string {
  if (bookmark.bookId !== book.id) throw new Error("This bookmark belongs to a different Library item.");
  const title = `${book.title} — ${bookmarkLabel(book, bookmark.position)}`
    .replace(/[\r\n]+/g, " ").replace(/[&<>\\[\]`*_#]/g, char => `&#${char.charCodeAt(0)};`);
  return `[${title}](${libraryLink(book.id, bookmark.id)})\n${bookmark.note ? `${bookmark.note}\n` : ""}`;
}
