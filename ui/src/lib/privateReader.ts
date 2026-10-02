import { invoke } from "@tauri-apps/api/core";
import { writable } from "svelte/store";
import { isBookLocation, type BookLocation } from "./bookLocations";
import { androidPrivateCommand, androidReaderRequest, isAndroidReader, normalizeAndroidReaderPosition } from "./privateReaderAndroid";

export interface ReaderProgress { position: number; total: number; anchor: string; label: string }
export interface ReaderPosition { trackId?: string; offsetMs: number; locator?: BookLocation; voiceId?: string }
export interface ReaderTrack { id: string; title: string; relativePath: string; durationMs?: number; available?: boolean }
export interface ReaderBookmark {
  id: string; bookId: string; position: ReaderPosition; createdAt: string | number; note: string;
}
export interface ReaderBook {
  id: string; title: string; kind: "audio" | "epub" | "video" | "youtube"; available: boolean;
  tracks: ReaderTrack[]; position: ReaderPosition | null; bookmarks: ReaderBookmark[];
  error?: string;
  favorite?: boolean; lastUsedAt?: number; sourceUrl?: string; progress?: ReaderProgress;
}
export interface ReaderSnapshot { libraryPath: string | null; books: ReaderBook[]; error?: string }
export const privateLibrary = writable<ReaderSnapshot>({ libraryPath: null, books: [] });
export const privateLibraryError = writable("");
export const privateBookJump = writable<{ bookId: string; locator: BookLocation } | null>(null);
export const privateVisualPositions = new Map<string, ReaderPosition>();
export const privateBookLanguages = writable<Record<string, string>>({});
export const privateVoiceLanguageSuggestion = writable<{ bookId: string; title: string; language: string } | null>(null);

export function readerNative<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isAndroidReader()) {
    if (["set_favorite", "record_activity", "add_link"].includes(command)) {
      return androidReaderRequest<ReaderSnapshot & { configured: boolean; locationLabel: string }>(command, args)
        .then(result => ({
          ...result, libraryPath: result.configured ? result.locationLabel || "Android local library" : null,
          books: result.books.map(book => ({ ...book, position: normalizeAndroidReaderPosition(book.position),
            bookmarks: book.bookmarks.map(mark => ({ ...mark, position: normalizeAndroidReaderPosition(mark.position)! })) })),
        }) as T);
    }
    // Network media belongs to the foreground Library player, never the SAF service.
    if (command === "save_position" && (args?.position as ReaderPosition)?.locator === undefined
      && !(args?.position as ReaderPosition)?.trackId) {
      return androidReaderRequest<T>("external_position", args);
    }
    if (command === "bookmark" && args?.position && !(args.position as ReaderPosition).locator
      && !(args.position as ReaderPosition).trackId) {
      return androidReaderRequest<T>("external_bookmark", args);
    }
    return androidPrivateCommand<T>(command, args);
  }
  return invoke<T>(`reader_${command === "bookmark" ? "add_bookmark" : command}`,
    command === "restore" ? { backup: args?.data } : args);
}

export function normalizePrivateLibraryLink(kind: "audio" | "video" | "youtube", value: string): string {
  if (!/^https?:\/\//i.test(value) || value.length > 8192 || /[\s\\\u0000-\u001f\u007f-\u009f]/.test(value)) throw new Error("Invalid Library URL.");
  let url: URL;
  try { url = new URL(value); } catch { throw new Error("Invalid Library URL."); }
  if (!["http:", "https:"].includes(url.protocol) || !url.hostname || url.username || url.password
    || value.split("://")[1]?.split(/[/?#]/)[0]?.includes("@")) throw new Error("Use HTTP(S) without credentials.");
  if (kind !== "youtube") return url.href;
  if (url.port) throw new Error("Invalid YouTube URL port.");
  const parts = url.pathname.split("/").slice(1);
  let id: string | null = null;
  if (["youtu.be", "www.youtu.be"].includes(url.hostname) && parts.length === 1) id = parts[0];
  else if (["youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "youtube-nocookie.com", "www.youtube-nocookie.com"].includes(url.hostname)) {
    if (url.pathname === "/watch") id = url.searchParams.get("v");
    else if (parts.length === 2 && ["embed", "shorts", "live"].includes(parts[0])) id = parts[1];
  }
  if (!id || !/^[A-Za-z0-9_-]{11}$/.test(id)) throw new Error("Use a known YouTube video URL.");
  return `https://www.youtube.com/watch?v=${id}`;
}

export function isReaderProgress(value: unknown): value is ReaderProgress {
  if (!value || typeof value !== "object") return false;
  const p = value as ReaderProgress;
  return Object.keys(value).length === 4 && Object.keys(value).every(key => ["position", "total", "anchor", "label"].includes(key))
    && Number.isFinite(p.position) && p.position >= 0 && p.position <= 31536000000
    && Number.isFinite(p.total) && p.total >= 0 && p.total <= 31536000000
    && (p.total === 0 || p.position <= p.total) && typeof p.anchor === "string" && p.anchor.length <= 8192
    && typeof p.label === "string" && p.label.length <= 1024 && !/[\u0000-\u001f\u007f-\u009f]/.test(p.anchor + p.label);
}

function validPosition(value: unknown): boolean {
  if (!value || typeof value !== "object") return false;
  const p = value as ReaderPosition;
  return Number.isSafeInteger(p.offsetMs) && p.offsetMs >= 0 && p.offsetMs <= 31536000000
    && (p.trackId === undefined || typeof p.trackId === "string")
    && (p.locator === undefined || (isBookLocation(p.locator) && p.locator.kind === "epub"))
    && (p.voiceId === undefined || (typeof p.voiceId === "string" && /^[A-Za-z0-9_-]{1,96}$/.test(p.voiceId)));
}

function acceptSnapshot(value: ReaderSnapshot): ReaderSnapshot {
  if (!value || (value.libraryPath !== null && typeof value.libraryPath !== "string") || !Array.isArray(value.books)
    || value.books.some(book => !book || typeof book.id !== "string" || !book.id || typeof book.title !== "string"
      || !["audio", "epub", "video", "youtube"].includes(book.kind) || typeof book.available !== "boolean"
      || !Array.isArray(book.tracks) || !Array.isArray(book.bookmarks)
      || book.tracks.some(track => !track || typeof track.id !== "string" || !track.id
        || typeof track.title !== "string" || typeof track.relativePath !== "string")
      || book.bookmarks.some(mark => !mark || typeof mark.id !== "string" || mark.bookId !== book.id
        || typeof mark.note !== "string" || !validPosition(mark.position))
      || (book.position !== null && !validPosition(book.position))
      || (book.favorite !== undefined && typeof book.favorite !== "boolean")
      || (book.lastUsedAt !== undefined && (!Number.isSafeInteger(book.lastUsedAt) || book.lastUsedAt < 0))
      || (book.progress !== undefined && !isReaderProgress(book.progress))
      || (book.sourceUrl !== undefined && (typeof book.sourceUrl !== "string" || book.kind === "epub"))
      || (book.kind === "youtube" && !book.sourceUrl))) throw new Error("Invalid native Library snapshot; current Library retained.");
  for (const book of value.books) {
    if (book.sourceUrl !== undefined) {
      if (normalizePrivateLibraryLink(book.kind as "audio" | "video" | "youtube", book.sourceUrl) !== book.sourceUrl
        || book.tracks.length || [book.position, ...book.bookmarks.map(mark => mark.position)].some(position =>
          position && (position.trackId !== undefined || position.locator !== undefined || position.voiceId !== undefined))) {
        throw new Error("Invalid external Library snapshot; current Library retained.");
      }
    }
  }
  return { ...value, books: value.books.map(book => ({ ...book, favorite: book.favorite ?? false, lastUsedAt: book.lastUsedAt ?? 0 })) };
}

let mutationTail: Promise<unknown> = Promise.resolve();
function privateMutation<T>(action: () => Promise<T>): Promise<T> {
  const mutation = mutationTail.then(async () => {
    ++refreshGeneration;
    try { return await action(); }
    finally { ++refreshGeneration; }
  });
  mutationTail = mutation.catch(() => {});
  return mutation;
}

function libraryMutation(command: string, args: Record<string, unknown>,
  validate?: (snapshot: ReaderSnapshot) => void): Promise<ReaderSnapshot> {
  return privateMutation(async () => {
    const snapshot = acceptSnapshot(await readerNative<ReaderSnapshot>(command, args));
    validate?.(snapshot);
    privateLibrary.set(snapshot);
    privateLibraryError.set(snapshot.error ?? "");
    return snapshot;
  });
}

export async function setPrivateFavorite(bookId: string, favorite: boolean): Promise<void> {
  await libraryMutation("set_favorite", { bookId, favorite }, snapshot => {
    if (!snapshot.books.some(book => book.id === bookId && book.favorite === favorite)) {
      throw new Error("Native Library did not confirm the favorite.");
    }
  });
}

export async function recordPrivateLibraryActivity(bookId: string, progress?: ReaderProgress): Promise<void> {
  if (progress !== undefined && !isReaderProgress(progress)) throw new Error("Invalid Library progress.");
  await libraryMutation("record_activity", { bookId, ...(progress === undefined ? {} : { progress }) }, snapshot => {
    if (!snapshot.books.some(book => book.id === bookId && (book.lastUsedAt ?? 0) > 0)) {
      throw new Error("Native Library did not confirm the activity.");
    }
  });
}

export async function addPrivateLibraryLink(title: string, kind: "youtube" | "audio" | "video", url: string): Promise<ReaderBook> {
  const normalized = normalizePrivateLibraryLink(kind, url);
  const snapshot = await libraryMutation("add_link", { title, kind, url: normalized }, value => {
    if (!value.books.some(book => book.kind === kind && book.sourceUrl === normalized)) {
      throw new Error("Native Library did not confirm the added link.");
    }
  });
  const book = snapshot.books.find(book => book.kind === kind && book.sourceUrl === normalized);
  if (!book) throw new Error("Native Library did not confirm the added link.");
  return book;
}

let refreshGeneration = 0;
export async function refreshPrivateLibrary(rescan = false): Promise<void> {
  const generation = ++refreshGeneration;
  try {
    const snapshot = acceptSnapshot(await readerNative<ReaderSnapshot>(rescan ? "rescan" : "snapshot"));
    if (generation === refreshGeneration) {
      privateLibrary.set(snapshot);
      privateLibraryError.set(snapshot.error ?? "");
    }
  } catch (cause) {
    if (generation === refreshGeneration) privateLibraryError.set(String(cause));
    throw cause;
  }
}

export async function savePrivatePosition(bookId: string, position: ReaderPosition): Promise<void> {
  await privateMutation(async () => {
    await readerNative("save_position", { bookId, position });
    privateLibrary.update(snapshot => ({
      ...snapshot, books: snapshot.books.map(book => book.id === bookId ? { ...book, position } : book),
    }));
  });
}

export async function savePrivateBookmark(bookId: string, position: ReaderPosition, note = "") {
  return privateMutation(async () => {
    const bookmark = await readerNative<ReaderBookmark>("bookmark", { bookId, position, note });
    await refreshPrivateLibrary();
    return bookmark;
  });
}

export function readerTime(ms: number): string {
  const seconds = Number.isFinite(ms) ? Math.max(0, Math.floor(ms / 1000)) : 0;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor(seconds / 60) % 60;
  return `${hours ? `${hours}:${String(minutes).padStart(2, "0")}` : minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

export function bookmarkLabel(book: ReaderBook, position: ReaderPosition): string {
  return position.locator ? "Saved EPUB passage" :
    `${book.sourceUrl ? book.title : book.tracks.find(track => track.id === position.trackId)?.title ?? "Unavailable chapter"} · ${readerTime(position.offsetMs)}`;
}

export function bookmarkDate(value: string | number): string {
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toISOString() : "";
}
