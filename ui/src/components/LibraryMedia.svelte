<script lang="ts">
  import { untrack } from "svelte";
  import { get } from "svelte/store";
  import LibrarySourcePlayer from "./LibrarySourcePlayer.svelte";
  import { readerNative, privateLibraryError, privateVisualPositions, type ReaderBook, type ReaderPosition } from "../lib/privateReader";
  import { libraryMediaRequest, saveLibraryCheckpoint, type LibraryProgress } from "../lib/library";
  import { pausePrivatePlayback, stopPrivatePlayback, privatePlayback, validatePrivateMediaURL } from "../lib/privateReaderPlayback";
  import { isAndroidReader } from "../lib/privateReaderAndroid";
  let { book, onPlayback, onActivity, onProgress }: {
    book: ReaderBook; onPlayback?: (playing: boolean) => void; onActivity?: () => void;
    onProgress?: (progress: LibraryProgress) => void;
  } = $props();
  let started = $state(false);
  let foregroundPlayer = $state<LibrarySourcePlayer>();
  let opening = $state(false);
  let error = $state("");
  let playerKey = $state(0);
  let initial = $state<ReaderPosition | null>(null);
  let pending: { id: string; position: ReaderPosition; progress: LibraryProgress; active: boolean } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastPosition = 0;
  const unsupported = $derived(isAndroidReader() && !book.sourceUrl);
  const sourceId = $derived(book.id);
  const item = $derived({
    id: book.id, title: book.title, kind: book.kind === "youtube" ? "youtube" as const : book.kind === "audio" ? "audio" as const : "video" as const,
    source: book.sourceUrl ?? book.id,
    progress: { position: (initial?.offsetMs ?? 0) / 1000, total: 0, anchor: "", label: "" },
  });
  async function start(position = book.position) {
    opening = true; error = "";
    try {
      if (get(privatePlayback).status === "loading") await stopPrivatePlayback();
      else if (get(privatePlayback).status === "playing") await pausePrivatePlayback();
      flush();
      initial = position; lastPosition = (position?.offsetMs ?? 0) / 1000; playerKey++; started = true;
    } catch (cause) { error = String(cause); }
    finally { opening = false; }
  }
  async function mediaURL() {
    const trackId = initial?.trackId ?? book.tracks[0]?.id;
    if (!trackId || !book.tracks.some(track => track.id === trackId && track.available !== false))
      throw new Error("The saved video source is missing. Relink it; a replacement was not guessed.");
    return validatePrivateMediaURL(await readerNative<string>("media_url", { bookId: book.id, trackId }));
  }
  function flush() {
    clearTimeout(timer); timer = undefined;
    const captured = pending; pending = null;
    if (!captured) return;
    void saveLibraryCheckpoint(captured.id, captured.position, captured.progress, captured.active).catch(cause => {
      error = `Library position not saved: ${String(cause)}`;
      privateLibraryError.set(error);
    });
  }
  function progress(value: LibraryProgress, active: boolean) {
    const position: ReaderPosition = {
      offsetMs: Math.round(value.position * 1000),
      ...(!book.sourceUrl ? { trackId: initial?.trackId ?? book.tracks[0]?.id } : {}),
    };
    privateVisualPositions.set(book.id, position);
    if (value.position === lastPosition) return;
    lastPosition = value.position;
    onProgress?.(value);
    pending = { id: book.id, position, progress: value, active };
    timer ??= setTimeout(flush, 1000);
  }
  function playback(playing: boolean) {
    onPlayback?.(playing);
    if (playing) onActivity?.();
    if (!playing) flush();
    else if (get(privatePlayback).status === "playing")
      void pausePrivatePlayback().catch(cause => { error = String(cause); });
  }
  $effect(() => {
    if ($privatePlayback.status === "playing") untrack(() => foregroundPlayer?.pausePlayback());
  });
  $effect(() => {
    const request = $libraryMediaRequest;
    if (request?.bookId === book.id && !unsupported) {
      untrack(() => { libraryMediaRequest.set(null); void start(request.position ?? book.position); });
    }
  });
  $effect(() => {
    const id = sourceId;
    return () => { flush(); privateVisualPositions.delete(id); };
  });
</script>

<section data-help-context="library" aria-label="Library media player">
  {#if unsupported}
    <p role="alert">Local video playback is not supported on Android yet. Your Library history is retained; play this source on desktop.</p>
  {:else if started}
    {#key playerKey}<LibrarySourcePlayer bind:this={foregroundPlayer} {item} autoplay={true} helpContext="library"
      resolveMedia={book.sourceUrl ? undefined : mediaURL} onProgress={progress} onPlayback={playback} />{/key}
    <p class="status">{book.kind === "audio" ? "Network audio on Android stops when you leave this player." : "Video and YouTube stop when you leave this player."}</p>
  {:else}
    <button disabled={opening || !book.available} onclick={() => start()}>{opening ? "Opening…" : book.position ? "Resume playback" : "Play"}</button>
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
</section>

<style>
  button { font: inherit; padding: 8px 12px; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; cursor: pointer; }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .status { font-size: 12px; color: var(--text-muted); }
  [role="alert"] { color: var(--danger, #c44); overflow-wrap: anywhere; }
</style>
