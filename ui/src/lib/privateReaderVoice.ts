import { invoke } from "@tauri-apps/api/core";
import { get } from "svelte/store";
import { BOOK_RENDERER_VERSION, type BookLocation } from "./bookLocations";
import { privateLibrary, savePrivateBookmark } from "./privateReader";
import { claimPrivateNarration, updatePrivateNarration } from "./privateReaderPlayback";
import { androidReaderRequest, isAndroidReader } from "./privateReaderAndroid";
import { canonicalNarrationBatches, collectPrivateSegments, startAndroidPrivateNarration, type CanonicalNarrationSegment } from "./privateReaderSegments";
import { saveLibraryCheckpoint } from "./library";
import { unavailableSourceMessage } from "./libraryLocations";
import { applyReaderPlaybackRate, speechPlaybackRate } from "./readerPlaybackPreferences";

export interface PrivateVoiceManifest {
  schema_version: number; id: string; name: string; language: string; runtime: string;
  license: string; license_url: string; sample_rate: number;
  artifacts: { path: string; role: string; bytes: number; sha256: string; url: string | null }[];
}
export interface PrivateVoiceStatus {
  available: boolean; runtime: string; reason: string | null;
  selection: { voice_id: string; language: string } | null;
  runtime_executable?: string | null;
  installed?: PrivateVoiceManifest[];
  installationErrors?: { id: string; available: false; error: string }[];
}
interface Clip { segment: CanonicalNarrationSegment; bytes: ArrayBuffer }

export async function voiceCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isAndroidReader()) {
    if (command === "status") return androidReaderRequest<T>("voiceStatus");
    if (command === "installed") {
      const status = await androidReaderRequest<PrivateVoiceStatus>("voiceStatus");
      return (status.installed ?? []) as T;
    }
    if (command === "import") return androidReaderRequest<T>("importVoice");
    if (command === "download") {
      if (args?.userAuthorized !== true) throw new Error("An explicit model download authorization is required.");
      return androidReaderRequest<T>("downloadVoice", { manifest: args.manifest, authorized: true });
    }
    if (command === "select") return androidReaderRequest<T>("selectVoice", { voiceId: args?.voiceId, language: args?.language });
    if (command === "cancel") return androidReaderRequest<T>("stop");
    throw new Error(`The Android voice adapter does not expose ${command}; no desktop or system speech fallback was attempted.`);
  }
  return invoke<T>(`private_voice_${command}`, args);
}

export function boundedVoiceAudio(value: unknown): ArrayBuffer {
  if (!(value instanceof ArrayBuffer) || value.byteLength < 44 || value.byteLength > 16 * 1024 * 1024)
    throw new Error("The offline worker returned invalid or oversized audio.");
  const header = new Uint8Array(value, 0, 12);
  if (String.fromCharCode(...header.subarray(0, 4)) !== "RIFF"
    || String.fromCharCode(...header.subarray(8, 12)) !== "WAVE")
    throw new Error("The offline worker did not return WAV audio.");
  return value;
}

let generation = 0;

/** Both platforms use the isolated renderer's exact, sanitized-document CFIs. */
export async function startPrivateReadAloud(bookId: string, locator?: BookLocation | boolean): Promise<void> {
  if (isAndroidReader()) return startAndroidPrivateNarration(bookId, locator === true,
    typeof locator === "object" ? locator : undefined);
  const book = get(privateLibrary).books.find(item => item.id === bookId);
  if (book?.disconnected) throw new Error(unavailableSourceMessage(book, "read aloud"));
  if (!book?.available || book.kind !== "epub") throw new Error("Choose an available private EPUB first.");
  const request = ++generation;
  const audio = new Audio();
  audio.preload = "auto";
  let stopped = false;
  let paused = false;
  let loading = true;
  let queue: CanonicalNarrationSegment[] = [];
  let active: CanonicalNarrationSegment | null = null;
  let voiceId = "";
  let objectURL = "";
  let timer: ReturnType<typeof setInterval> | undefined;
  let saving = Promise.resolve();
  let lastPosition = "";
  let played = false;
  const pending = new Map<number, Promise<Clip>>();
  let stopRate = () => {};
  const current = () => active ? {
    offsetMs: Math.max(0, Math.round(audio.currentTime * 1000)), locator: active.locator, voiceId,
  } : null;
  const checkpoint = () => {
    if (!active || !audio.src) return saving;
    const position = current()!;
    const key = JSON.stringify(position);
    const moved = played && key !== lastPosition;
    const progress = { position: active.ordinal + (Number.isFinite(audio.duration) && audio.duration > 0 ? Math.min(1, audio.currentTime / audio.duration) : 0),
      total: queue.length, anchor: active.locator.kind === "epub" ? active.locator.cfi : "", label: `Read aloud · passage ${active.ordinal + 1} of ${queue.length}` };
    lastPosition = key;
    saving = saving.catch(() => {}).then(async () => {
      await saveLibraryCheckpoint(bookId, position, progress, moved);
    });
    return saving;
  };
  const fail = (cause: unknown) => {
    if (stopped || request !== generation) return;
    paused = true; audio.pause();
    updatePrivateNarration({ status: "paused", error: String(cause) });
  };
  const fetchClip = (ordinal: number): Promise<Clip> => {
    const existing = pending.get(ordinal);
    if (existing) return existing;
    if (pending.size >= 2) throw new Error("Offline speech prefetch exceeded its two-clip limit.");
    const promise = (async () => {
      const segment = queue[ordinal];
      if (stopped || request !== generation) throw new Error("Narration cancelled.");
      if (!segment) throw new Error("The canonical narration segment is unavailable.");
      const clip = await voiceCommand<{ file_name: string }>("synthesize", {
        text: segment.text, requestId: crypto.randomUUID(),
      });
      if (stopped || request !== generation) throw new Error("Narration cancelled.");
      const bytes = boundedVoiceAudio(await voiceCommand<ArrayBuffer>("audio", { fileName: clip.file_name }));
      return { segment, bytes };
    })();
    pending.set(ordinal, promise);
    void promise.catch(() => {}); // A prefetched failure is surfaced when its clip is reached.
    return promise;
  };
  const load = async (ordinal: number, offsetMs = 0) => {
    loading = true;
    updatePrivateNarration({ status: "loading", error: "" });
    const clip = await fetchClip(ordinal);
    pending.delete(ordinal);
    if (stopped || request !== generation) return;
    active = clip.segment;
    if (objectURL) URL.revokeObjectURL(objectURL);
    objectURL = URL.createObjectURL(new Blob([clip.bytes], { type: "audio/wav" }));
    audio.src = objectURL;
    await new Promise<void>((resolve, reject) => {
      const cleanup = () => {
        clearTimeout(timeout); audio.removeEventListener("loadedmetadata", ready); audio.removeEventListener("error", failed);
      };
      const ready = () => { cleanup(); resolve(); };
      const failed = () => { cleanup(); reject(new Error("Generated offline audio could not be loaded.")); };
      const timeout = setTimeout(() => { cleanup(); reject(new Error("Generated offline audio timed out.")); }, 15000);
      audio.addEventListener("loadedmetadata", ready); audio.addEventListener("error", failed); audio.load();
    });
    if (stopped || request !== generation) return;
    if (!Number.isFinite(audio.duration) || offsetMs / 1000 > audio.duration + 1)
      throw new Error("The saved voice clip offset is incompatible; restart narration from the beginning.");
    audio.currentTime = Math.min(offsetMs / 1000, audio.duration);
    applyReaderPlaybackRate(audio, get(speechPlaybackRate));
    loading = false;
    played = false;
    await checkpoint();
    if (stopped || request !== generation) return;
    updatePrivateNarration({ position: current(), status: paused ? "paused" : "playing", playbackRate: audio.playbackRate });
    if (!paused) { await audio.play(); played = true; }
    if (ordinal + 1 < queue.length) void fetchClip(ordinal + 1);
  };
  await claimPrivateNarration(book, {
    setRate: rate => { applyReaderPlaybackRate(audio, rate); },
    pause: async () => { paused = true; audio.pause(); await checkpoint(); },
    resume: async () => {
      if (loading || !audio.src) throw new Error("Wait for the native voice to prepare, or restart after an error.");
      paused = false; await audio.play(); played = true;
    },
    stop: async () => {
      stopped = true; audio.pause(); clearInterval(timer); stopRate();
      if (request === generation) ++generation;
      try { await checkpoint(); }
      finally {
        audio.removeAttribute("src"); audio.load(); pending.clear();
        if (objectURL) URL.revokeObjectURL(objectURL);
        await voiceCommand("cancel");
      }
    },
    bookmark: async () => {
      const position = current();
      if (loading || !position) throw new Error("Wait for the narration paragraph to begin before bookmarking.");
      await checkpoint();
      await savePrivateBookmark(bookId, position);
    },
  });
  if (stopped || request !== generation) return;
  stopRate = speechPlaybackRate.subscribe(rate => {
    if (stopped || request !== generation) return;
    try { applyReaderPlaybackRate(audio, rate); updatePrivateNarration({ playbackRate: audio.playbackRate }); }
    catch (cause) { fail(cause); }
  });
  audio.addEventListener("error", () => fail(new Error("Offline speech playback failed.")));
  audio.addEventListener("ended", () => {
    void (async () => {
      await checkpoint();
      if (stopped || !active) return;
      if (active.ordinal + 1 >= queue.length) {
        paused = true; clearInterval(timer); stopRate();
        updatePrivateNarration({ status: "stopped" });
        return;
      }
      await load(active.ordinal + 1);
    })().catch(fail);
  });
  try {
    const status = await voiceCommand<PrivateVoiceStatus>("status");
    if (!status.available) throw new Error(status.reason || "The offline speech runtime is unavailable.");
    if (!status.selection) throw new Error("Select an installed voice in Settings → Library location.");
    voiceId = status.selection.voice_id;
    queue = canonicalNarrationBatches(await collectPrivateSegments(bookId)).flat();
    if (stopped || request !== generation) return;
    const seen = new Set<string>();
    for (const segment of queue) {
      if (segment.locator.kind !== "epub" || segment.locator.rendererVersion !== BOOK_RENDERER_VERSION
        || seen.has(segment.locator.cfi))
        throw new Error("The narration queue needs distinct canonical reader CFIs. Reload the EPUB before reading aloud.");
      seen.add(segment.locator.cfi);
    }
    const saved = get(privateLibrary).books.find(item => item.id === bookId)?.position;
    const start = typeof locator === "object" ? locator : locator === true ? undefined : saved?.locator;
    const ordinal = start ? queue.findIndex(segment => segment.locator.kind === "epub" && start.kind === "epub"
      && segment.locator.cfi === start.cfi && segment.locator.rendererVersion === start.rendererVersion) : 0;
    if (ordinal < 0)
      throw new Error("The saved passage is not an exact narration segment. Start narration from the beginning explicitly.");
    const resumeOffset = locator === true || typeof locator === "object" || saved?.voiceId !== voiceId ? 0 : saved?.offsetMs ?? 0;
    timer = setInterval(() => {
      if (!stopped && !paused && !loading) {
        updatePrivateNarration({ position: current() });
        void checkpoint().catch(fail);
      }
    }, 4000);
    await load(ordinal, resumeOffset);
  } catch (cause) { fail(cause); throw cause; }
}
