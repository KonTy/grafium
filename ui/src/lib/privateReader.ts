import { invoke } from "@tauri-apps/api/core";
import { writable } from "svelte/store";
import type { BookLocation } from "./bookLocations";
import { androidPrivateCommand, isAndroidReader } from "./privateReaderAndroid";

export interface ReaderPosition { trackId?: string; offsetMs: number; locator?: BookLocation; voiceId?: string }
export interface ReaderTrack { id: string; title: string; relativePath: string; durationMs?: number; available?: boolean }
export interface ReaderBookmark {
  id: string; bookId: string; position: ReaderPosition; createdAt: string | number; note: string;
}
export interface ReaderBook {
  id: string; title: string; kind: "audio" | "epub"; available: boolean;
  tracks: ReaderTrack[]; position: ReaderPosition | null; bookmarks: ReaderBookmark[];
  error?: string;
}
export interface ReaderSnapshot { libraryPath: string | null; books: ReaderBook[]; error?: string }
export const privateLibrary = writable<ReaderSnapshot>({ libraryPath: null, books: [] });
export const privateLibraryError = writable("");
export const privateBookJump = writable<{ bookId: string; locator: BookLocation } | null>(null);
export const privateVisualPositions = new Map<string, ReaderPosition>();
export const privateBookLanguages = writable<Record<string, string>>({});
export const privateVoiceLanguageSuggestion = writable<{ bookId: string; title: string; language: string } | null>(null);

export function readerNative<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isAndroidReader()) return androidPrivateCommand<T>(command, args);
  return invoke<T>(`reader_${command === "bookmark" ? "add_bookmark" : command}`,
    command === "restore" ? { backup: args?.data } : args);
}

let refreshGeneration = 0;
export async function refreshPrivateLibrary(rescan = false): Promise<void> {
  const generation = ++refreshGeneration;
  try {
    const snapshot = await readerNative<ReaderSnapshot>(rescan ? "rescan" : "snapshot");
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
  await readerNative("save_position", { bookId, position });
  privateLibrary.update(snapshot => ({
    ...snapshot, books: snapshot.books.map(book => book.id === bookId ? { ...book, position } : book),
  }));
}

export async function savePrivateBookmark(bookId: string, position: ReaderPosition, note = "") {
  const bookmark = await readerNative<ReaderBookmark>("bookmark", { bookId, position, note });
  await refreshPrivateLibrary();
  return bookmark;
}

export function readerTime(ms: number): string {
  const seconds = Number.isFinite(ms) ? Math.max(0, Math.floor(ms / 1000)) : 0;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor(seconds / 60) % 60;
  return `${hours ? `${hours}:${String(minutes).padStart(2, "0")}` : minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

export function bookmarkLabel(book: ReaderBook, position: ReaderPosition): string {
  return position.locator ? "Saved EPUB passage" :
    `${book.tracks.find(track => track.id === position.trackId)?.title ?? "Unavailable chapter"} · ${readerTime(position.offsetMs)}`;
}

export function bookmarkDate(value: string | number): string {
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toISOString() : "";
}
