import { invoke } from "@tauri-apps/api/core";
import { writable } from "svelte/store";
import type { Page } from "./api";
import { BOOK_RENDERER_VERSION, isBookLocation, type BookLocation } from "./bookLocations";
export { BOOK_RENDERER_VERSION, isBookLocation, type BookLocation, type BookRect } from "./bookLocations";

export type BookFormat = "epub" | "fb2" | "mobi" | "azw3" | "pdf";
export interface BookInfo {
  id: string; pageId: string; title: string; format: BookFormat; filePath: string;
  sourceSha256: string; readingLocation: BookLocation | null; indexingWarning: string | null;
}
export interface BookNote {
  id: string; bookId: string; notePageId: string; filePath: string; body: string; quote: string;
  locator: BookLocation | null; sourceSha256: string; revision: string;
  createdAt: string; updatedAt: string; status: "attached" | "orphaned";
}
export interface BookSelection {
  graphPath: string; bookId: string; pageId: string; sourceSha256: string;
  quote: string; locator: BookLocation;
}
export interface BookJump extends Omit<BookSelection, "quote"> { requestId: string }
export const bookSelection = writable<BookSelection | null>(null);
export const bookJump = writable<BookJump | null>(null);
export const bookNoteChanges = writable(0);
export function notifyBookNotesChanged() { bookNoteChanges.update(n => n + 1); }

export function isOriginalBookPage(page: Page): boolean {
  const p = page.properties;
  return typeof p["book-id"] === "string" && !!p["book-id"]
    && ["epub", "fb2", "mobi", "azw3", "pdf"].includes(String(p["book-format"]))
    && typeof p["book-source-sha256"] === "string" && !!p["book-source-sha256"]
    && typeof p["book-source"] === "string" && !!p["book-source"];
}
export function compatibleBookLocation(book: BookInfo, location: BookLocation): boolean {
  return isBookLocation(location) && (book.format === "pdf" ? location.kind === "pdf"
    : location.kind === "epub" && location.rendererVersion === BOOK_RENDERER_VERSION);
}
export function selectionForBook(selection: BookSelection | null, graphPath: string, book: BookInfo): BookSelection | null {
  return selection?.graphPath === graphPath && selection.bookId === book.id && selection.pageId === book.pageId
    && selection.sourceSha256 === book.sourceSha256 && selection.quote.trim()
    && compatibleBookLocation(book, selection.locator) ? structuredClone(selection) : null;
}
export function jumpToBookNote(graphPath: string, book: BookInfo, note: BookNote): void {
  if (note.bookId !== book.id || note.sourceSha256 !== book.sourceSha256 || note.status !== "attached"
    || !note.locator || !compatibleBookLocation(book, note.locator)) {
    throw new Error("This passage is unavailable: the source or reader version changed.");
  }
  bookJump.set({ graphPath, bookId: book.id, pageId: book.pageId, sourceSha256: book.sourceSha256,
    locator: structuredClone(note.locator), requestId: crypto.randomUUID() });
}
export const bookOpen = (graphPath: string, pageId: string) =>
  invoke<BookInfo>("book_open", { graphPath, pageId });
export const bookReadBytes = (graphPath: string, bookId: string) =>
  invoke<ArrayBuffer>("book_read_bytes", { graphPath, bookId });
export const bookSavePosition = (graphPath: string, bookId: string, sourceSha256: string, location: BookLocation) =>
  invoke<void>("book_save_position", { graphPath, bookId, sourceSha256, location });
export const bookNotesList = (graphPath: string, bookId: string) =>
  invoke<BookNote[]>("book_notes_list", { graphPath, bookId });
export const bookNoteSave = (args: {
  graphPath: string; bookId: string; noteId: string; expectedRevision: string | null;
  body: string; quote: string; locator: BookLocation | null; sourceSha256: string;
}) => invoke<BookNote>("book_note_save", args);
export const bookNoteDelete = (graphPath: string, bookId: string, noteId: string, expectedRevision: string) =>
  invoke<void>("book_note_delete", { graphPath, bookId, noteId, expectedRevision });
