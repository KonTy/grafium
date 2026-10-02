import { isBookLocation, type BookLocation } from "./bookLocations";
import type { ReaderBook, ReaderPosition, ReaderSnapshot } from "./privateReader";

declare global {
  interface Window { PrivateReaderBridge?: { request(json: string): void } }
}
export const isAndroidReader = () => typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);
export interface AndroidReaderState {
  bookId: string | null; trackId: string | null; offsetMs: number;
  playing: boolean; buffering: boolean; error: string | null; durationMs: number;
  mode?: "audio" | "tts"; ttsLoading?: boolean; ordinal?: number; segmentCount?: number;
  locator?: BookLocation | string | null;
}
export interface ReaderVolumeCapabilities {
  volume: {
    settings: { enabled: boolean; key: "up" | "down"; gesture: "longPress" };
    accessibilityConnected: boolean; lockedScreen: string; screenOff: string; verification: string;
  };
}

export function androidReaderRequest<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  const bridge = window.PrivateReaderBridge;
  if (!bridge) return Promise.reject(new Error("The native Android private reader bridge is unavailable. This WebView cannot safely play books in the background."));
  return new Promise<T>((resolve, reject) => {
    const id = crypto.randomUUID();
    const cleanup = () => { clearTimeout(timer); window.removeEventListener("private-reader-response", receive); };
    const receive = (event: Event) => {
      const response = (event as CustomEvent).detail;
      if (!response || response.id !== id) return;
      cleanup();
      if (response.ok === true) resolve(response.result as T);
      else reject(new Error(typeof response.error === "string" ? response.error : "Native private reader request failed."));
    };
    // Pickers require human interaction and should not inherit short playback timeouts.
    const timeout = ["pickLocation", "exportState", "restoreState", "importState", "importVoice", "downloadVoice"].includes(command) ? 600000 : 60000;
    const timer = setTimeout(() => { cleanup(); reject(new Error(`Native private reader ${command} timed out. No success was confirmed.`)); }, timeout);
    window.addEventListener("private-reader-response", receive);
    try { bridge.request(JSON.stringify({ id, command, args })); }
    catch (cause) { cleanup(); reject(cause); }
  });
}

export function normalizeAndroidReaderPosition(value: { trackId?: string; offsetMs: number; locator?: unknown } | null): ReaderPosition | null {
  if (!value) return null;
  let locator = value.locator;
  if (typeof locator === "string" && locator.trimStart().startsWith("{")) {
    try { locator = JSON.parse(locator); }
    catch { throw new Error("The native narration locator is invalid; it was not guessed."); }
  }
  if (isBookLocation(locator) && locator.kind === "epub") {
    return { offsetMs: value.offsetMs, locator };
  }
  if (locator !== undefined && locator !== null) throw new Error("Unsupported native reader locator.");
  return { offsetMs: value.offsetMs, ...(value.trackId ? { trackId: value.trackId } : {}) };
}
function normalizeBook(book: ReaderBook): ReaderBook {
  return {
    ...book, position: normalizeAndroidReaderPosition(book.position),
    bookmarks: book.bookmarks.map(mark => ({
      ...mark, bookId: book.id, position: normalizeAndroidReaderPosition(mark.position)!,
    })),
  };
}
export async function androidPrivateCommand<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (command === "snapshot" || command === "rescan") {
    const result = await androidReaderRequest<{ configured: boolean; locationLabel: string; books: ReaderBook[]; error?: string }>(command === "snapshot" ? "library" : "rescan");
    // Unavailable records still render even if the provider is disconnected.
    return { libraryPath: result.configured ? result.locationLabel || "Android local library" : null,
      books: result.books.map(normalizeBook), ...(result.error ? { error: result.error } : {}) } as ReaderSnapshot as T;
  }
  if (command === "read_epub") {
    const { base64 } = await androidReaderRequest<{ base64: string }>("readEpub", args);
    return Uint8Array.from(atob(base64), character => character.charCodeAt(0)).buffer as T;
  }
  if (command === "save_position") {
    const position = args.position as ReaderPosition;
    if (!position.locator || position.locator.kind !== "epub")
      throw new Error("Android listening positions belong to the native playback service.");
    return androidReaderRequest<T>("position", { bookId: args.bookId, locator: position.locator, offsetMs: position.offsetMs });
  }
  if (command === "bookmark" && (args.position as ReaderPosition | undefined)?.locator) {
    return androidReaderRequest<T>("bookmarkVisual", args);
  }
  const names: Record<string, string> = { bookmark: "bookmark", update_bookmark: "updateBookmark", delete_bookmark: "deleteBookmark", reorder: "reorder", relink: "relink", restore: "restore" };
  if (names[command]) return androidReaderRequest<T>(names[command], args);
  throw new Error(`The Android reader does not support ${command}. No desktop fallback was attempted.`);
}
