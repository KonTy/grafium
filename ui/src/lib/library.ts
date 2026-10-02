import { bookmarkLabel, normalizePrivateLibraryLink, recordPrivateLibraryActivity, savePrivatePosition, type ReaderBook, type ReaderPosition, type ReaderProgress } from "./privateReader";
import { studySourceFromLink, webStudyUrl, youtubeVideoId } from "./studySources";
import { writable } from "svelte/store";

export type LibraryProgress = ReaderProgress;
export const libraryMediaRequest = writable<{ bookId: string; position?: ReaderPosition; nonce: number } | null>(null);
export function requestLibraryMedia(bookId: string, position?: ReaderPosition) {
  libraryMediaRequest.set({ bookId, position, nonce: Date.now() });
}

export function libraryBooks(books: ReaderBook[], query = "", favorites = false, kind = "all"): ReaderBook[] {
  const search = query.trim().toLocaleLowerCase();
  return books.filter(book => (!favorites || book.favorite)
    && (kind === "all" || book.kind === kind)
    && book.title.toLocaleLowerCase().includes(search))
    .sort((a, b) => (b.lastUsedAt ?? 0) - (a.lastUsedAt ?? 0)
      || a.title.localeCompare(b.title) || a.id.localeCompare(b.id));
}

export function libraryPercent(book: ReaderBook): number | null {
  const progress = book.progress;
  return progress && Number.isFinite(progress.total) && progress.total > 0 && Number.isFinite(progress.position)
    ? Math.round(Math.min(1, Math.max(0, progress.position / progress.total)) * 100) : null;
}

export function libraryPositionLabel(book: ReaderBook): string {
  if (book.progress?.label) return book.progress.label;
  return book.position ? bookmarkLabel(book, book.position) : "Not started";
}

export function libraryLink(value: string, title = "", kind: "auto" | "audio" | "video" = "auto"): {
  title: string; kind: "audio" | "video" | "youtube"; url: string;
} {
  const url = webStudyUrl(value);
  const inferred = studySourceFromLink(url.href);
  if (inferred.kind === "youtube") return {
    title: title.trim() || `YouTube · ${youtubeVideoId(inferred.source)}`, kind: "youtube",
    url: normalizePrivateLibraryLink("youtube", inferred.source),
  };
  const selected = kind === "auto" ? inferred.kind : kind;
  if (selected !== "audio" && selected !== "video")
    throw new Error("Use a YouTube video or a direct audio/video URL. For extensionless media, choose Audio or Video.");
  return { title: title.trim() || inferred.filenameTitle || url.hostname, kind: selected, url: normalizePrivateLibraryLink(selected, url.href) };
}

// One queue spans visual reading, narration, and media so an old checkpoint can
// never finish after a newer checkpoint from another view of the same source.
let writing = Promise.resolve();
export function saveLibraryCheckpoint(bookId: string, position: ReaderPosition, progress?: LibraryProgress, active = true): Promise<void> {
  const next = writing.catch(() => {}).then(async () => {
    await savePrivatePosition(bookId, position);
    if (active) await recordPrivateLibraryActivity(bookId, progress ? {
      ...progress, anchor: progress.anchor.length <= 8192 ? progress.anchor.replace(/[\u0000-\u001f\u007f-\u009f]/g, "") : "",
      label: progress.label.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").slice(0, 1024),
    } : undefined);
  });
  writing = next;
  return next;
}
