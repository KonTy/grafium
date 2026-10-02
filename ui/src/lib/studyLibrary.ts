import { bookmarkLabel, type ReaderBook } from "./privateReader";
import type { StudyItem, StudyProgress } from "./studies";

export function studyClockItem(item: StudyItem, books: ReaderBook[]): StudyItem {
  if (item.kind !== "library") return item;
  const book = books.find(book => book.id === item.source);
  if (!book) throw new Error("This Library item is unavailable on this device.");
  return { ...item, kind: book.kind === "epub" ? "book" : book.kind };
}

export function studyDisplayProgress(item: StudyItem, books: ReaderBook[]): StudyProgress {
  if (item.kind !== "library") return item.progress;
  const book = books.find(book => book.id === item.source);
  if (!book) return { position: 0, total: 0, anchor: "", label: "Library item unavailable on this device" };
  if (book.progress) return book.progress;
  return {
    position: book.position ? book.position.offsetMs / 1000 : 0, total: 0, anchor: "",
    label: book.position ? bookmarkLabel(book, book.position) : book.available ? "" : "Source unavailable",
  };
}
