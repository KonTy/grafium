import { get, writable } from "svelte/store";
import { privateLibrary, readerNative, savePrivateBookmark, type ReaderBook, type ReaderPosition } from "./privateReader";
import { androidReaderRequest, isAndroidReader, normalizeAndroidReaderPosition, type AndroidReaderState } from "./privateReaderAndroid";
import { refreshPrivateLibrary } from "./privateReader";
import { saveLibraryCheckpoint, type LibraryProgress } from "./library";
import { webStudyUrl } from "./studySources";
import { applyReaderPlaybackRate, mediaPlaybackRate, speechPlaybackRate, setMediaPlaybackRate, setSpeechPlaybackRate, validateReaderPlaybackRate } from "./readerPlaybackPreferences";

export interface ReaderPlaybackState {
  bookId: string | null; title: string; mode: "audio" | "tts"; status: "stopped" | "playing" | "paused" | "loading";
  position: ReaderPosition | null; error: string;
  durationMs?: number; seekable?: boolean; playbackRate?: number;
}
export interface PrivateNarrationAdapter {
  pause(): Promise<void>; resume(): Promise<void>; stop(): Promise<void>;
  bookmark(): Promise<void>;
  setRate?(rate: number): void;
}
export const privatePlayback = writable<ReaderPlaybackState>({
  bookId: null, title: "", mode: "audio", status: "stopped", position: null, error: "",
});

// A single app-owned player survives component unmounts and graph switches.
let audio: HTMLAudioElement | null = null;
let narration: PrivateNarrationAdapter | null = null;
let generation = 0;
let writing = Promise.resolve();
let activeBook: ReaderBook | null = null;
let activityPosition: ReaderPosition | null = null;
let cancelPreparation: (() => void) | null = null;
let cancelAudioLoad: (() => void) | null = null;

export function registerPrivatePreparation(cancel: () => void): () => void {
  cancelPreparation?.();
  cancelPreparation = cancel;
  return () => { if (cancelPreparation === cancel) cancelPreparation = null; };
}

function patch(value: Partial<ReaderPlaybackState>) { privatePlayback.update(state => ({ ...state, ...value })); }
function fail(cause: unknown) { patch({ error: String(cause) }); }
mediaPlaybackRate.subscribe(rate => {
  if (!audio || isAndroidReader() || get(privatePlayback).mode !== "audio" || get(privatePlayback).status === "stopped") return;
  try { applyReaderPlaybackRate(audio, rate); patch({ playbackRate: audio.playbackRate }); }
  catch (cause) { fail(cause); }
});
function updateAudioProgress() {
  const state = get(privatePlayback);
  if (!audio?.src || state.mode !== "audio" || state.status === "stopped") return;
  const durationMs = Number.isFinite(audio.duration) && audio.duration > 0 ? Math.round(audio.duration * 1000) : 0;
  patch({ position: position(), durationMs, seekable: durationMs > 0 && audio.seekable.length > 0 });
}
function position(): ReaderPosition | null {
  const state = get(privatePlayback);
  return state.mode === "audio" && audio && state.position
    ? { ...state.position, offsetMs: Math.max(0, Math.round(audio.currentTime * 1000)) } : state.position;
}
export function checkpointPrivatePlayback(): Promise<void> {
  if (isAndroidReader()) return Promise.resolve(); // The Media3 service owns durable checkpoints.
  const state = get(privatePlayback);
  const captured = position();
  if (!state.bookId || !captured || state.mode !== "audio") return writing;
  patch({ position: captured });
  const bookId = state.bookId;
  const moved = state.status === "playing" && captured.offsetMs !== activityPosition?.offsetMs;
  const book = get(privateLibrary).books.find(item => item.id === bookId) ?? activeBook;
  const track = book?.tracks.find(item => item.id === captured.trackId);
  const total = audio && Number.isFinite(audio.duration) ? audio.duration : 0;
  const progress: LibraryProgress = {
    position: captured.offsetMs / 1000, total: (book?.tracks.length ?? 0) <= 1 ? total : 0,
    anchor: captured.trackId ?? "", label: `${track?.title ? `${track.title} · ` : ""}${Math.floor(captured.offsetMs / 60000)}:${String(Math.floor(captured.offsetMs / 1000) % 60).padStart(2, "0")}`,
  };
  const next = writing.catch(() => {}).then(async () => {
    await saveLibraryCheckpoint(bookId, captured, progress, moved);
    if (moved && get(privatePlayback).bookId === bookId) activityPosition = captured;
  });
  writing = next;
  return next;
}
function player(): HTMLAudioElement {
  if (audio) return audio;
  audio = new Audio();
  audio.preload = "metadata";
  for (const event of ["loadedmetadata", "durationchange", "progress", "timeupdate", "seeked"])
    audio.addEventListener(event, updateAudioProgress);
  audio.addEventListener("error", () => {
    patch({ status: "paused", error: `Playback failed (${audio?.error?.code ?? "unknown"}). Check that the library source is still accessible.` });
  });
  audio.addEventListener("ended", () => {
    void (async () => {
      await checkpointPrivatePlayback();
      const current = get(privatePlayback);
      const book = get(privateLibrary).books.find(item => item.id === activeBook?.id) ?? activeBook;
      const index = book?.tracks.findIndex(track => track.id === current.position?.trackId) ?? -1;
      if (book && index >= 0 && index + 1 < book.tracks.length)
        await playPrivateAudio(book, { trackId: book.tracks[index + 1].id, offsetMs: 0 });
      else await stopPrivatePlayback();
    })().catch(fail);
  });
  setInterval(() => {
    if (get(privatePlayback).status === "playing") void checkpointPrivatePlayback().catch(fail);
  }, 4000);
  window.addEventListener("pagehide", () => { void checkpointPrivatePlayback().catch(fail); });
  return audio;
}

export function validatePrivateMediaURL(value: string): string {
  const url = new URL(value);
  if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || !url.port
    || url.username || url.password || url.hash)
    throw new Error("The private reader refused a non-loopback media URL.");
  return url.href;
}

export async function playPrivateAudio(book: ReaderBook, saved = book.position): Promise<void> {
  if (!book.available) throw new Error("This source is unavailable. Relink it before playing.");
  if (book.kind !== "audio") throw new Error("This book is not an audiobook.");
  const remote = book.sourceUrl ? webStudyUrl(book.sourceUrl).href : null;
  if (remote && isAndroidReader()) throw new Error("Open this network audio in Library. Android network media uses the foreground player, not the offline audio service.");
  const trackId = remote ? undefined : saved?.trackId ?? book.tracks[0]?.id;
  if (!remote && (!trackId || !book.tracks.some(track => track.id === trackId && track.available !== false)))
    throw new Error("The saved chapter is missing. Choose a chapter explicitly; progress was not guessed.");
  await stopPrivatePlayback();
  const request = ++generation;
  activeBook = book;
  const selected = { trackId, offsetMs: saved?.offsetMs ?? 0 };
  activityPosition = selected;
  patch({ bookId: book.id, title: book.title, mode: "audio", status: "loading", position: selected, error: "", durationMs: 0, seekable: false, playbackRate: undefined });
  try {
    if (isAndroidReader()) {
      const state = await androidReaderRequest<AndroidReaderState>("play", { bookId: book.id, ...selected, playbackRate: get(mediaPlaybackRate) });
      if (request === generation) applyAndroidState(state);
      return;
    }
    const url = remote ?? validatePrivateMediaURL(await readerNative<string>("media_url", { bookId: book.id, trackId }));
    if (request !== generation) return;
    const element = player();
    applyReaderPlaybackRate(element, get(mediaPlaybackRate));
    element.src = url;
    await new Promise<void>((resolve, reject) => {
      const cleanup = () => {
        element.removeEventListener("loadedmetadata", ready); element.removeEventListener("error", failed); clearTimeout(timeout);
        if (cancelAudioLoad === ready) cancelAudioLoad = null;
      };
      const ready = () => { cleanup(); resolve(); };
      const failed = () => { cleanup(); reject(new Error("The audio source could not be loaded.")); };
      const timeout = setTimeout(() => { cleanup(); reject(new Error("Timed out loading the audio source.")); }, 30000);
      element.addEventListener("loadedmetadata", ready, { once: true });
      element.addEventListener("error", failed, { once: true });
      cancelAudioLoad = ready;
      element.load();
    });
    if (request !== generation) return;
    element.currentTime = selected.offsetMs / 1000;
    applyReaderPlaybackRate(element, get(mediaPlaybackRate));
    patch({ playbackRate: element.playbackRate });
    updateAudioProgress();
    await element.play();
    if (request === generation) patch({ status: "playing" });
  } catch (cause) {
    if (request !== generation) return;
    audio?.pause(); patch({ status: "paused", error: String(cause) });
    throw cause;
  }
}

export async function pausePrivatePlayback(): Promise<void> {
  const request = generation;
  if (narration) await narration.pause();
  else if (isAndroidReader()) {
    const state = await androidReaderRequest<AndroidReaderState>("pause");
    if (request === generation) applyAndroidState(state);
    return;
  }
  else { audio?.pause(); try { await checkpointPrivatePlayback(); } finally { if (request === generation) patch({ status: "paused" }); } }
  if (request === generation) patch({ status: "paused" });
}
export async function setPrivatePlaybackRate(rate: number): Promise<void> {
  validateReaderPlaybackRate(rate);
  const state = get(privatePlayback);
  const request = generation;
  if (!state.bookId || state.status === "stopped") throw new Error("Start playback before changing its speed.");
  if (isAndroidReader()) {
    const next = await androidReaderRequest<AndroidReaderState>("setPlaybackRate", { rate });
    if (request !== generation) throw new Error("Playback changed before its speed was confirmed.");
    if (typeof next.playbackRate !== "number" || !Number.isFinite(next.playbackRate)
      || Math.abs(next.playbackRate - rate) > 0.001 || next.bookId !== state.bookId)
      throw new Error("The native player did not confirm the requested playback speed.");
    applyAndroidState(next);
  } else if (narration) {
    if (!narration.setRate) throw new Error("This narration player cannot change speed.");
    narration.setRate(rate);
    patch({ playbackRate: rate });
  } else {
    if (!audio?.src) throw new Error("No audio is ready for a speed change.");
    applyReaderPlaybackRate(audio, rate);
    patch({ playbackRate: audio.playbackRate });
  }
  if (state.mode === "tts") setSpeechPlaybackRate(rate);
  else setMediaPlaybackRate(rate);
}
export async function configurePrivateSpeechRate(rate: number): Promise<void> {
  validateReaderPlaybackRate(rate);
  const state = get(privatePlayback);
  if (state.bookId && state.mode === "tts" && state.status !== "stopped") await setPrivatePlaybackRate(rate);
  else setSpeechPlaybackRate(rate);
}
export async function resumePrivatePlayback(): Promise<void> {
  const request = generation;
  if (narration) await narration.resume();
  else if (isAndroidReader()) {
    const state = await androidReaderRequest<AndroidReaderState>("resume");
    if (request === generation) applyAndroidState(state);
    return;
  }
  else if (audio?.src) await audio.play();
  else throw new Error("No reader is ready to resume. Open the book again.");
  if (request === generation) patch({ status: "playing", error: "" });
}
export async function stopPrivatePlayback(): Promise<void> {
  ++generation;
  cancelAudioLoad?.();
  cancelPreparation?.();
  cancelPreparation = null;
  if (narration) {
    const previous = narration;
    try { await previous.stop(); }
    finally { narration = null; patch({ status: "stopped" }); }
  }
  else if (isAndroidReader() && get(privatePlayback).bookId) {
    applyAndroidState(await androidReaderRequest<AndroidReaderState>("stop"));
    patch({ status: "stopped" });
    return;
  }
  if (audio?.src) {
    audio.pause();
    const checkpoint = checkpointPrivatePlayback();
    audio.removeAttribute("src"); audio.load(); patch({ status: "stopped" });
    await checkpoint;
  }
  patch({ status: "stopped" });
}
export async function skipPrivateAudio(deltaMs: number): Promise<void> {
  await seekPrivateAudioPosition((position()?.offsetMs ?? 0) + deltaMs);
}
export async function seekPrivateAudioPosition(offsetMs: number): Promise<void> {
  const state = get(privatePlayback);
  if (!Number.isFinite(offsetMs)) throw new Error("The requested playback position is invalid.");
  if (narration || state.mode !== "audio" || !state.seekable || !state.durationMs || state.status === "stopped")
    throw new Error("Seeking is unavailable until this player reports a seekable duration.");
  const target = Math.max(0, Math.min(state.durationMs, offsetMs));
  if (isAndroidReader()) {
    const request = generation;
    const next = await androidReaderRequest<AndroidReaderState>("seek", {
      offsetMs: Math.round(target),
    });
    if (request === generation) applyAndroidState(next);
    return;
  }
  if (narration || !audio?.src || get(privatePlayback).mode !== "audio")
    throw new Error("Seeking is only available for the active audiobook.");
  const seconds = target / 1000;
  let available = false;
  for (let index = 0; index < audio.seekable.length; index++)
    if (seconds >= audio.seekable.start(index) && seconds <= audio.seekable.end(index)) available = true;
  if (!available) throw new Error("This position is outside the media's available seek range.");
  audio.currentTime = seconds;
  updateAudioProgress();
  await checkpointPrivatePlayback();
}
export async function bookmarkPrivatePlayback(): Promise<void> {
  if (narration) return narration.bookmark();
  if (isAndroidReader()) {
    // Deliberately omit the UI position: capture, save, and haptic are atomic in the service.
    await androidReaderRequest("bookmark", { bookId: get(privatePlayback).bookId });
    await refreshPrivateLibrary();
    return;
  }
  const state = get(privatePlayback);
  const captured = position();
  if (!state.bookId || !captured) throw new Error("No active book position to bookmark.");
  await savePrivateBookmark(state.bookId, captured);
}

/** Native narration owns its queue and progress; never use browser speech synthesis. */
export async function claimPrivateNarration(book: ReaderBook, adapter: PrivateNarrationAdapter): Promise<void> {
  await stopPrivatePlayback();
  narration = adapter;
  patch({ bookId: book.id, title: book.title, mode: "tts", status: "loading", position: book.position, error: "", durationMs: 0, seekable: false, playbackRate: undefined });
}
export function updatePrivateNarration(update: Partial<ReaderPlaybackState>): void { patch(update); }

export function applyAndroidState(state: AndroidReaderState): void {
  if (narration || !state || typeof state.playing !== "boolean") return;
  const book = get(privateLibrary).books.find(item => item.id === state.bookId);
  let nativePosition: ReaderPosition | null = null;
  let locatorError = "";
  try {
    nativePosition = normalizeAndroidReaderPosition(state.mode === "tts"
      ? { offsetMs: state.offsetMs, locator: state.locator }
      : state.trackId ? { trackId: state.trackId, offsetMs: state.offsetMs } : null);
  } catch (cause) { locatorError = String(cause); }
  let rate = 1;
  if ("playbackRate" in state && state.playbackRate !== undefined) {
    try {
      if (typeof state.playbackRate !== "number") throw new Error("Native playback speed is invalid.");
      validateReaderPlaybackRate(state.playbackRate); rate = state.playbackRate;
    } catch (cause) { locatorError = String(cause); }
  }
  patch({
    bookId: state.bookId, title: (book?.title ?? (get(privatePlayback).bookId === state.bookId ? get(privatePlayback).title : "")) || "Background playback",
    mode: state.mode === "tts" ? "tts" : "audio",
    status: state.buffering || state.ttsLoading ? "loading" : state.playing ? "playing" : state.bookId ? "paused" : "stopped",
    position: nativePosition, error: state.error || locatorError,
    durationMs: state.mode !== "tts" && Number.isFinite(state.durationMs) && state.durationMs > 0 ? state.durationMs : 0,
    seekable: state.mode !== "tts" && state.seekable === true && state.durationMs > 0,
    playbackRate: rate,
  });
}

export function attachPrivatePlayback(): () => void {
  if (!isAndroidReader()) return () => {};
  const receive = (event: Event) => applyAndroidState((event as CustomEvent<AndroidReaderState>).detail);
  const restore = () => {
    void androidReaderRequest<AndroidReaderState>("state").then(applyAndroidState).catch(fail);
    void refreshPrivateLibrary().then(() => {
      const book = get(privateLibrary).books.find(item => item.id === get(privatePlayback).bookId);
      if (book) patch({ title: book.title });
    }).catch(fail);
  };
  window.addEventListener("private-reader-state", receive);
  window.addEventListener("focus", restore);
  restore();
  return () => {
    window.removeEventListener("private-reader-state", receive);
    window.removeEventListener("focus", restore);
  };
}

export async function seekPrivateAudio(book: ReaderBook, target: ReaderPosition): Promise<void> {
  await playPrivateAudio(get(privateLibrary).books.find(item => item.id === book.id) ?? book, target);
}
