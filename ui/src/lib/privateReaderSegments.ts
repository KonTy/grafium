import type { ReaderTextSegment } from "./bookReaderSecurity";
import { isBookLocation, type BookLocation } from "./bookLocations";
import { androidReaderRequest, type AndroidReaderState } from "./privateReaderAndroid";
import { applyAndroidState, registerPrivatePreparation, stopPrivatePlayback, updatePrivateNarration } from "./privateReaderPlayback";
import { privateLibrary, savePrivatePosition } from "./privateReader";
import { get } from "svelte/store";
import { speechPlaybackRate } from "./readerPlaybackPreferences";
import { unavailableSourceMessage } from "./libraryLocations";

type SegmentSource = () => Promise<ReaderTextSegment[]>;
const sources = new Map<string, { collect: SegmentSource; sourceHash: () => string }>();
export function registerPrivateSegments(bookId: string, source: SegmentSource, sourceHash: () => string = () => ""): () => void {
  const registration = { collect: source, sourceHash };
  sources.set(bookId, registration);
  return () => { if (sources.get(bookId) === registration) sources.delete(bookId); };
}
export function collectPrivateSegments(bookId: string): Promise<ReaderTextSegment[]> {
  const source = sources.get(bookId);
  if (!source) return Promise.reject(new Error("Open this EPUB and wait for it to load before starting read aloud."));
  return source.collect();
}

export interface CanonicalNarrationSegment extends ReaderTextSegment { ordinal: number }

export function canonicalNarrationBatches(segments: ReaderTextSegment[]): CanonicalNarrationSegment[][] {
  if (!segments.length || segments.length > 200000) throw new Error("The narration queue has an invalid segment count.");
  const encoder = new TextEncoder();
  const batches: CanonicalNarrationSegment[][] = [];
  let batch: CanonicalNarrationSegment[] = [];
  let batchBytes = 2;
  let totalBytes = 0;
  for (const [ordinal, segment] of segments.entries()) {
    if (!segment.text.trim() || encoder.encode(segment.text).byteLength > 1500
      || !isBookLocation(segment.locator) || segment.locator.kind !== "epub")
      throw new Error("The isolated reader returned an invalid narration segment.");
    const item = { ordinal, text: segment.text, locator: segment.locator };
    const bytes = encoder.encode(JSON.stringify(item)).byteLength + 1;
    totalBytes += bytes;
    if (bytes > 500000 || totalBytes > 64 * 1024 * 1024)
      throw new Error("The complete narration queue exceeds its safety limit. No truncated book was started.");
    if (batch.length === 256 || batchBytes + bytes > 500000) {
      batches.push(batch); batch = []; batchBytes = 2;
    }
    batch.push(item); batchBytes += bytes;
  }
  if (batch.length) batches.push(batch);
  return batches;
}

/** Upload completely before native Start: the service never depends on a live WebView queue. */
export async function startAndroidPrivateNarration(bookId: string, fromBeginning = false, locator?: BookLocation): Promise<void> {
  const book = get(privateLibrary).books.find(item => item.id === bookId);
  if (book?.disconnected) throw new Error(unavailableSourceMessage(book, "read aloud"));
  if (!book?.available || book.kind !== "epub") throw new Error("Open an available private EPUB before starting native narration.");
  const status = await androidReaderRequest<{ available: boolean; reason?: string; selection: unknown }>("voiceStatus");
  if (!status.available) throw new Error(status.reason || "The native offline speech engine is unavailable.");
  if (!status.selection) throw new Error("Select an installed offline voice in Library settings first.");
  if (locator && (!isBookLocation(locator) || locator.kind !== "epub"))
    throw new Error("Android narration needs a canonical EPUB locator.");
  await stopPrivatePlayback();
  if (locator) await savePrivatePosition(bookId, { offsetMs: 0, locator });
  let cancelled = false;
  let uploadId = "";
  let committed = false;
  const release = registerPrivatePreparation(() => { cancelled = true; });
  const check = () => { if (cancelled) throw new Error("Native narration preparation was cancelled."); };
  updatePrivateNarration({ bookId, title: book.title, mode: "tts", status: "loading", position: book.position, error: "" });
  try {
    const upload = await androidReaderRequest<{ uploadId: string; sourceHash: string }>("narrationBegin", { bookId });
    uploadId = upload.uploadId;
    if (!uploadId || !/^[a-fA-F0-9]{64}$/.test(upload.sourceHash)) throw new Error("The native narration source was not verified.");
    const displayedHash = sources.get(bookId)?.sourceHash();
    if (!displayedHash || displayedHash.toLowerCase() !== upload.sourceHash.toLowerCase())
      throw new Error("The displayed EPUB no longer matches its registered source. Reload the book before reading aloud.");
    check();
    const segments = await collectPrivateSegments(bookId);
    check();
    for (const batch of canonicalNarrationBatches(segments)) {
      await androidReaderRequest("narrationAppend", { uploadId, segments: batch });
      check();
    }
    await androidReaderRequest("narrationCommit", { uploadId });
    committed = true;
    check();
    const state = await androidReaderRequest<AndroidReaderState>("narrationStart", {
      bookId, fromBeginning, playbackRate: get(speechPlaybackRate),
    });
    check();
    applyAndroidState(state);
  } catch (cause) {
    let failure = cause;
    if (uploadId && !committed) {
      try { await androidReaderRequest("narrationCancel", { uploadId }); }
      catch (cleanupError) {
        const message = cleanupError instanceof Error ? cleanupError.message : String(cleanupError);
        if (!/^NARRATION_UPLOAD_NOT_FOUND\b/.test(message)) {
          failure = new Error(`${String(cause)} Upload cleanup also failed: ${message}`);
          if (cancelled) updatePrivateNarration({ error: String(failure) });
        }
      }
    }
    if (!cancelled) updatePrivateNarration({ status: "paused", error: String(failure) });
    throw failure;
  } finally { release(); }
}
