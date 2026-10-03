import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type LibraryIndexSemantic = "ready" | "unavailable" | "stale" | "embedding";
export type LibraryIndexTranscription = "ready" | "off" | "unavailable";
export type LibrarySearchKind = "audio" | "video" | "epub" | "youtube";
export type LibrarySearchMatch = "keyword" | "semantic" | "both" | "title";

export interface LibraryIndexStatus {
  enabled: boolean;
  transcribeMedia: boolean;
  running: boolean;
  jobId: string | null;
  items: { total: number; indexed: number; pending: number; failed: number; titleOnly: number };
  chunks: number;
  semantic: LibraryIndexSemantic;
  semanticReason: string | null;
  transcription: LibraryIndexTranscription;
  transcriptionReason: string | null;
  lastIndexedAt: number | null;
  errors: { bookId: string; title: string; message: string }[];
}

export interface LibrarySearchHit {
  bookId: string;
  title: string;
  chunkId: string | null;
  kind: LibrarySearchKind;
  snippet: string;
  trackId: string | null;
  startMs: number | null;
  endMs: number | null;
  chapter: string | null;
  quote: string | null;
  score: number;
  match: LibrarySearchMatch;
}

export interface LibrarySourceDto {
  index: number;
  book_id: string;
  title: string;
  kind: LibrarySearchKind;
  track_id: string | null;
  start_ms: number | null;
  end_ms: number | null;
  chapter: string | null;
  quote: string | null;
}

export interface LibrarySource {
  index: number;
  bookId: string;
  title: string;
  kind: LibrarySearchKind;
  trackId: string | null;
  startMs: number | null;
  endMs: number | null;
  chapter: string | null;
  quote: string | null;
}

export const FALLBACK_LIBRARY_INDEX_STATUS: LibraryIndexStatus = {
  enabled: true,
  transcribeMedia: true,
  running: false,
  jobId: null,
  items: { total: 0, indexed: 0, pending: 0, failed: 0, titleOnly: 0 },
  chunks: 0,
  semantic: "unavailable",
  semanticReason: "Library index status is unavailable.",
  transcription: "unavailable",
  transcriptionReason: "Library index status is unavailable.",
  lastIndexedAt: null,
  errors: [],
};

const semantics = new Set<LibraryIndexSemantic>(["ready", "unavailable", "stale", "embedding"]);
const transcriptions = new Set<LibraryIndexTranscription>(["ready", "off", "unavailable"]);
const kinds = new Set<LibrarySearchKind>(["audio", "video", "epub", "youtube"]);
const matches = new Set<LibrarySearchMatch>(["keyword", "semantic", "both", "title"]);
const CONTROL = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g;

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function finiteInteger(value: unknown, min = 0, max = Number.MAX_SAFE_INTEGER): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= min && value <= max;
}

function truncateText(value: string, max: number): string {
  const cleaned = value.replace(CONTROL, " ").replace(/\s+/g, " ").trim();
  return cleaned.length <= max ? cleaned : `${cleaned.slice(0, Math.max(0, max - 1)).trimEnd()}…`;
}

function optionalString(value: unknown, max = 8192): string | null {
  if (value === null || value === undefined) return null;
  return typeof value === "string" ? truncateText(value, max) : null;
}

function requiredString(value: unknown, max = 8192): string | null {
  if (typeof value !== "string") return null;
  const cleaned = truncateText(value, max);
  return cleaned ? cleaned : null;
}

export function acceptLibraryIndexStatus(value: unknown): LibraryIndexStatus {
  if (!isRecord(value) || !isRecord(value.items)) throw new Error("Invalid Library index status.");
  const items = value.items;
  if (typeof value.enabled !== "boolean" || typeof value.transcribeMedia !== "boolean" || typeof value.running !== "boolean"
    || !(value.jobId === null || typeof value.jobId === "string") || !finiteInteger(value.chunks, 0, 1_000_000_000)
    || !semantics.has(value.semantic as LibraryIndexSemantic) || !transcriptions.has(value.transcription as LibraryIndexTranscription)
    || !(value.lastIndexedAt === null || finiteInteger(value.lastIndexedAt, 0, 4_102_444_800_000))
    || !finiteInteger(items.total, 0, 10_000_000) || !finiteInteger(items.indexed, 0, 10_000_000)
    || !finiteInteger(items.pending, 0, 10_000_000) || !finiteInteger(items.failed, 0, 10_000_000)
    || !finiteInteger(items.titleOnly, 0, 10_000_000)) {
    throw new Error("Invalid Library index status.");
  }
  const rawErrors = Array.isArray(value.errors) ? value.errors.slice(0, 20) : [];
  const status: LibraryIndexStatus = {
    enabled: value.enabled,
    transcribeMedia: value.transcribeMedia,
    running: value.running,
    jobId: value.jobId,
    items: { total: items.total, indexed: items.indexed, pending: items.pending, failed: items.failed, titleOnly: items.titleOnly },
    chunks: value.chunks,
    semantic: value.semantic as LibraryIndexSemantic,
    semanticReason: optionalString(value.semanticReason, 500),
    transcription: value.transcription as LibraryIndexTranscription,
    transcriptionReason: optionalString(value.transcriptionReason, 500),
    lastIndexedAt: value.lastIndexedAt,
    errors: rawErrors.flatMap((error) => {
      if (!isRecord(error)) return [];
      const bookId = requiredString(error.bookId, 512);
      const title = requiredString(error.title, 200);
      const message = requiredString(error.message, 500);
      return bookId && title && message ? [{ bookId, title, message }] : [];
    }),
  };
  if (status.items.indexed + status.items.pending + status.items.failed + status.items.titleOnly > Math.max(status.items.total * 2, status.items.total + 100)) {
    throw new Error("Invalid Library index status.");
  }
  return status;
}

export function acceptLibrarySearchHit(value: unknown): LibrarySearchHit | null {
  if (!isRecord(value)) return null;
  if (!kinds.has(value.kind as LibrarySearchKind) || !matches.has(value.match as LibrarySearchMatch)
    || !Number.isFinite(value.score) || typeof value.score !== "number") return null;
  const startMs = value.startMs;
  const endMs = value.endMs;
  if (!(startMs === null || finiteInteger(startMs, 0, 31_536_000_000)) || !(endMs === null || finiteInteger(endMs, 0, 31_536_000_000))) return null;
  if (startMs !== null && endMs !== null && endMs < startMs) return null;
  const bookId = requiredString(value.bookId, 512);
  const title = requiredString(value.title, 1024);
  const snippet = requiredString(value.snippet, 300);
  if (!bookId || !title || !snippet) return null;
  return {
    bookId,
    title,
    chunkId: optionalString(value.chunkId, 512),
    kind: value.kind as LibrarySearchKind,
    snippet,
    trackId: optionalString(value.trackId, 512),
    startMs,
    endMs,
    chapter: optionalString(value.chapter, 1024),
    quote: optionalString(value.quote, 200),
    score: value.score,
    match: value.match as LibrarySearchMatch,
  };
}

export function acceptLibrarySearchHits(value: unknown): LibrarySearchHit[] {
  if (!Array.isArray(value)) throw new Error("Invalid Library search results.");
  return value.slice(0, 100).flatMap((item) => {
    const hit = acceptLibrarySearchHit(item);
    return hit ? [hit] : [];
  });
}

export function acceptLibrarySource(value: unknown): LibrarySource | null {
  if (!isRecord(value) || !finiteInteger(value.index, 1, 1000) || !kinds.has(value.kind as LibrarySearchKind)) return null;
  const startMs = value.start_ms;
  const endMs = value.end_ms;
  if (!(startMs === null || finiteInteger(startMs, 0, 31_536_000_000)) || !(endMs === null || finiteInteger(endMs, 0, 31_536_000_000))) return null;
  const bookId = requiredString(value.book_id, 512);
  const title = requiredString(value.title, 1024);
  if (!bookId || !title) return null;
  return {
    index: value.index,
    bookId,
    title,
    kind: value.kind as LibrarySearchKind,
    trackId: optionalString(value.track_id, 512),
    startMs,
    endMs,
    chapter: optionalString(value.chapter, 1024),
    quote: optionalString(value.quote, 200),
  };
}

export function acceptLibrarySources(value: unknown): LibrarySource[] {
  if (!Array.isArray(value)) return [];
  return value.slice(0, 100).flatMap((item) => {
    const source = acceptLibrarySource(item);
    return source ? [source] : [];
  });
}

export async function libraryIndexStatus(): Promise<LibraryIndexStatus> {
  return acceptLibraryIndexStatus(await invoke("library_index_status"));
}

export async function libraryIndexSettingsSet(enabled: boolean, transcribeMedia: boolean): Promise<LibraryIndexStatus> {
  return acceptLibraryIndexStatus(await invoke("library_index_settings_set", { enabled, transcribeMedia }));
}

export async function libraryIndexStart(rebuild = false): Promise<string> {
  const job = await invoke("library_index_start", { rebuild });
  if (typeof job !== "string" || !job.trim() || job.length > 512) throw new Error("Invalid Library index job id.");
  return job;
}

export async function librarySearch(query: string, limit = 30): Promise<LibrarySearchHit[]> {
  const trimmed = query.trim();
  if (!trimmed) return [];
  const bounded = Math.max(1, Math.min(100, Math.floor(limit)));
  return acceptLibrarySearchHits(await invoke("library_search", { query: trimmed, limit: bounded }));
}

export function subscribeLibraryIndexUpdated(handler: (status: LibraryIndexStatus) => void, onError?: (message: string) => void): Promise<UnlistenFn> {
  return listen<unknown>("library-index-updated", (event) => {
    try { handler(acceptLibraryIndexStatus(event.payload)); }
    catch (cause) { onError?.(String(cause)); }
  });
}

export interface StaleLibrarySearchGuard {
  (query: string, limit?: number): Promise<LibrarySearchHit[] | null>;
  cancel(): void;
}

export function createStaleLibrarySearchGuard(searcher = librarySearch): StaleLibrarySearchGuard {
  let generation = 0;
  const guard = (async (query: string, limit?: number) => {
    const current = ++generation;
    try {
      const results = await searcher(query, limit);
      return current === generation ? results : null;
    } catch (cause) {
      if (current !== generation) return null;
      throw cause;
    }
  }) as StaleLibrarySearchGuard;
  guard.cancel = () => { generation += 1; };
  return guard;
}

export function isLibraryIndexOffError(cause: unknown): boolean {
  return String(cause).trim() === "Library index is off" || String(cause).includes("Library index is off");
}

export function formatLibraryTimestamp(ms: number | null | undefined): string {
  const seconds = Number.isFinite(ms) ? Math.max(0, Math.floor((ms ?? 0) / 1000)) : 0;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor(seconds / 60) % 60;
  const rest = seconds % 60;
  return hours ? `${hours}:${String(minutes).padStart(2, "0")}:${String(rest).padStart(2, "0")}` : `${minutes}:${String(rest).padStart(2, "0")}`;
}

export function formatLibrarySourcePosition(source: Pick<LibrarySource | LibrarySearchHit, "startMs" | "chapter" | "kind">): string {
  if ((source.kind === "audio" || source.kind === "video" || source.kind === "youtube") && source.startMs !== null && source.startMs !== undefined) {
    return formatLibraryTimestamp(source.startMs);
  }
  return source.chapter?.trim() || (source.kind === "epub" ? "EPUB passage" : "Title match");
}

export function formatLibraryLastRun(value: number | null): string {
  if (value === null) return "Never";
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toLocaleString() : "Unknown";
}
