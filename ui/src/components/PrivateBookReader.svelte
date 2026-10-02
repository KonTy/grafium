<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import ReadingSurface from "./ReadingSurface.svelte";
  import { observeReaderTheme, readReaderTheme } from "../lib/bookReaderTheme";
  import { get } from "svelte/store";
  import { BOOK_FRAME_SANDBOX, readReaderMessage, readerFrameURL, type BookTocItem } from "../lib/bookReaderSecurity";
  import { privateBookJump, privateLibrary, privateVisualPositions, readerNative, privateLibraryError, refreshPrivateLibrary, privateBookLanguages, privateVoiceLanguageSuggestion } from "../lib/privateReader";
  import type { BookLocation } from "../lib/bookLocations";
  import type { ReaderTextSegment, ReaderMessage } from "../lib/bookReaderSecurity";
  import { registerPrivateSegments } from "../lib/privateReaderSegments";
  import { privatePlayback } from "../lib/privateReaderPlayback";
  import { sha256 } from "@noble/hashes/sha256";
  import { saveLibraryCheckpoint, type LibraryProgress } from "../lib/library";
  let { bookId, onActivity, onProgress, actions, bookmarks, status, onBack, onBookmark }: {
    bookId: string; onActivity?: () => void; onProgress?: (progress: LibraryProgress) => void;
    actions?: Snippet; bookmarks?: Snippet; status?: Snippet; onBack?: () => void; onBookmark?: () => void;
  } = $props();
  let surface = $state<ReadingSurface>();
  let frame = $state<HTMLIFrameElement>();
  let url = $state("");
  let ready = $state(false);
  let error = $state("");
  let label = $state("");
  let fraction = $state<number | undefined>();
  let toc = $state<BookTocItem[]>([]);
  let size = $state(100);
  let retry = $state(0);
  let send: (type: string, data?: Record<string, unknown>) => void = () => {};
  let bootstrap = () => {};

  $effect(() => {
    const id = bookId;
    retry;
    const book = get(privateLibrary).books.find(item => item.id === id);
    const token = crypto.randomUUID();
    let disposed = false;
    let sourceAvailable = true;
    let bytes: ArrayBuffer | null = null;
    let runtime = "";
    let sourceHash = "";
    let pending: BookLocation | null = null;
    let pendingActivity = false;
    let navigationPending = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let openTimer: ReturnType<typeof setTimeout> | undefined;
    let writing = Promise.resolve();
    const requests = new Map<string, { resolve: (message: Extract<ReaderMessage, { type: "read-aloud-segments" }>) => void; reject: (cause: Error) => void; timer: ReturnType<typeof setTimeout> }>();
    ready = false; url = ""; error = ""; label = ""; fraction = undefined; toc = [];
    function flush() {
      clearTimeout(timer);
      const playback = get(privatePlayback);
      if (playback.bookId === id && playback.mode === "tts" && playback.status !== "stopped") pending = null;
      if (!sourceAvailable) pending = null;
      if (!pending) return writing;
      const locator = pending;
      const active = pendingActivity;
      const progress = { position: fraction ?? 0, total: fraction === undefined ? 0 : 1,
        anchor: locator.kind === "epub" ? locator.cfi : "", label };
      pending = null;
      pendingActivity = false;
      writing = writing.then(() => saveLibraryCheckpoint(id, { offsetMs: 0, locator }, progress, active)).catch(cause => {
        // A failed write is never shown as a saved place.
        if (!disposed) { if (!pending) { pending = locator; pendingActivity = active; } error = `Position not saved: ${String(cause)}`; }
        else privateLibraryError.set(`Private book position was not saved: ${String(cause)}`);
      });
      return writing;
    }
    send = (type, data = {}) => {
      if (["prev", "next", "toc", "goto"].includes(type)) navigationPending = true;
      else if (type === "size") navigationPending = false;
      if (!disposed) frame?.contentWindow?.postMessage({ channel: "grafium-book", token, type, ...data }, "*");
    };
    const unregisterSegments = registerPrivateSegments(id, async () => {
      if (!ready) throw new Error("Wait for the isolated EPUB reader to finish loading.");
      const segments: ReaderTextSegment[] = [];
      let section = 0; let offset = 0; let size = 0;
      while (!disposed) {
        const requestId = crypto.randomUUID();
        const response = await new Promise<Extract<ReaderMessage, { type: "read-aloud-segments" }>>((resolve, reject) => {
          const timer = setTimeout(() => { requests.delete(requestId); reject(new Error("Read-aloud extraction timed out.")); }, 30000);
          requests.set(requestId, { resolve, reject, timer });
          send("read-aloud-segments", { requestId, section, offset });
        });
        if (response.section !== section || response.nextOffset !== null && response.nextOffset <= offset)
          throw new Error("Invalid read-aloud segment sequence.");
        for (const segment of response.segments) {
          size += segment.text.length;
          if (size > 16 * 1024 * 1024 || segments.length >= 100000)
            throw new Error("This EPUB exceeds the read-aloud queue safety limit. No truncated narration was started.");
          segments.push(segment);
        }
        if (response.nextOffset !== null) offset = response.nextOffset;
        else if (++section < response.sectionCount) offset = 0;
        else return segments;
      }
      throw new Error("The EPUB was closed before its narration queue was prepared.");
    }, () => sourceHash);
    bootstrap = () => {
      if (!bytes || disposed) return;
      send("bootstrap", { runtime });
      frame?.contentWindow?.postMessage({ channel: "grafium-book", token, type: "open",
        bytes, format: "epub", location: book?.position?.locator }, "*", [bytes]);
      bytes = null;
    };
    const receive = (event: MessageEvent) => {
      const message = readReaderMessage(event, frame?.contentWindow ?? null, token);
      if (!message || disposed) return;
      if (message.type === "read-aloud-segments") {
        const request = requests.get(message.requestId);
        if (request) { clearTimeout(request.timer); requests.delete(message.requestId); request.resolve(message); }
      }
      if (message.type === "ready") {
        clearTimeout(openTimer); ready = true; toc = message.toc;
        send("theme", { theme: readReaderTheme() });
        if (message.language) {
          privateBookLanguages.update(languages => ({ ...languages, [id]: message.language! }));
          privateVoiceLanguageSuggestion.set({ bookId: id, title: book?.title ?? "Private EPUB", language: message.language });
        } else {
          privateBookLanguages.update(languages => {
            const next = { ...languages }; delete next[id]; return next;
          });
          privateVoiceLanguageSuggestion.set(null);
        }
      }
      else if (message.type === "error") {
        navigationPending = false;
        clearTimeout(openTimer); error = message.message;
        for (const request of requests.values()) { clearTimeout(request.timer); request.reject(new Error(message.message)); }
        requests.clear();
      }
      else if (message.type === "help") void surface?.exitFullscreen().then(() =>
        frame?.dispatchEvent(new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true })));
      else if (message.type === "toggle-controls") surface?.toggleControls();
      else if (message.type === "toggle-fullscreen") void surface?.toggleFullscreen();
      else if (message.type === "exit-fullscreen") void surface?.dismiss();
      else if (message.type === "selection" && sourceAvailable) onActivity?.();
      else if (message.type === "navigation") navigationPending = true;
      else if (message.type === "location" && message.location.kind === "epub") {
        if (!sourceAvailable) return;
        const explicitNavigation = navigationPending;
        navigationPending = false;
        label = message.label;
        fraction = message.fraction;
        const playback = get(privatePlayback);
        if (playback.bookId === id && playback.mode === "tts" && playback.status !== "stopped") {
          return;
        }
        privateVisualPositions.set(id, { offsetMs: 0, locator: message.location });
        const saved = get(privateLibrary).books.find(item => item.id === id)?.position;
        if (!explicitNavigation && saved?.locator && (saved.voiceId !== undefined || saved.offsetMs > 0)) return;
        pending = message.location;
        pendingActivity ||= explicitNavigation;
        if (explicitNavigation) {
          onActivity?.();
          onProgress?.({ position: message.fraction ?? 0, total: message.fraction === undefined ? 0 : 1,
            anchor: message.location.cfi, label: message.label });
        }
        clearTimeout(timer); timer = setTimeout(() => { void flush(); }, 600);
      }
    };
    window.addEventListener("message", receive);
    const stopTheme = untrack(() => observeReaderTheme(theme => send("theme", { theme })));
    window.addEventListener("pagehide", flush);
    const checkSource = () => {
      void refreshPrivateLibrary(true).then(() => {
        if (disposed) return;
        if (!get(privateLibrary).books.find(item => item.id === id)?.available) {
          sourceAvailable = false; pending = null; ready = false;
          error = "This source is no longer available. The displayed EPUB is a read-only snapshot; relink or restore the source, then reload.";
        }
      }).catch(cause => { if (!disposed) error = `Could not verify the private EPUB source: ${String(cause)}`; });
    };
    window.addEventListener("focus", checkSource);
    void (async () => {
      try {
        if (!book?.available) throw new Error("This private book source is unavailable. Relink it before reading.");
        const [data, response] = await Promise.all([readerNative<ArrayBuffer>("read_epub", { bookId: id }), fetch("/book-reader/runtime.js")]);
        if (!response.ok) throw new Error("The bundled offline EPUB runtime is unavailable.");
        runtime = await response.text();
        if (disposed) return;
        if (!(data instanceof ArrayBuffer)) throw new Error("The private reader did not receive binary EPUB bytes.");
        const digest = crypto.subtle ? await crypto.subtle.digest("SHA-256", data) : sha256(new Uint8Array(data));
        sourceHash = [...new Uint8Array(digest)].map(value => value.toString(16).padStart(2, "0")).join("");
        if (disposed) return;
        bytes = data; url = readerFrameURL(token, readReaderTheme());
        openTimer = setTimeout(() => { if (!disposed) error = "The isolated reader did not initialize. Reload to retry."; }, 45000);
      } catch (cause) { if (!disposed) error = String(cause); }
    })();
    return () => {
      disposed = true; void flush(); clearTimeout(openTimer);
      unregisterSegments();
      for (const request of requests.values()) { clearTimeout(request.timer); request.reject(new Error("Private reader closed during narration preparation.")); }
      requests.clear();
      privateVisualPositions.delete(id);
      window.removeEventListener("message", receive); window.removeEventListener("pagehide", flush);
      window.removeEventListener("focus", checkSource);
      stopTheme();
    };
  });
  $effect(() => {
    const jump = $privateBookJump;
    if (ready && jump?.bookId === bookId) {
      send("goto", { location: jump.locator });
      privateBookJump.set(null);
    }
  });
</script>

<section class="private-book" data-help-context="reader" aria-label="Private EPUB reader">
  <ReadingSurface bind:this={surface} {actions} {bookmarks} {onBack} {onBookmark}>
  {#snippet navigation()}
    <button disabled={!ready} onclick={() => send("prev")}>Previous</button>
    <button disabled={!ready} onclick={() => send("next")}>Next</button>
    <select disabled={!ready} aria-label="Private book contents" value="" onchange={event => {
      const item = toc[Number(event.currentTarget.value)];
      if (item) send("toc", { target: item.target });
      event.currentTarget.value = "";
    }}><option disabled value="">Contents…</option>{#each toc as item, index}<option value={index}>{"—".repeat(item.depth)} {item.label}</option>{/each}</select>
    <label>Text size<select bind:value={size} disabled={!ready} onchange={event => { onActivity?.(); send("size", { value: Number(event.currentTarget.value) }); }}>{#each [75, 100, 125, 150, 175, 200] as value}<option {value}>{value}%</option>{/each}</select></label>
    <small>{label}</small>
  {/snippet}
  {#snippet children()}
  {@render status?.()}
  {#if error}<p role="alert">{error} <button onclick={() => retry++}>Reload book</button></p>{/if}
  {#if !ready && !error}<p role="status">Opening isolated offline EPUB…</p>{/if}
  {#if url}<iframe bind:this={frame} src={url} sandbox={BOOK_FRAME_SANDBOX} title="Private EPUB content" onload={() => bootstrap()}></iframe>{/if}
  {/snippet}
  </ReadingSurface>
</section>

<style>
  .private-book { display: flex; flex-direction: column; min-height: 0; height: 100%; flex: 1; }
  button, select { font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; padding: 7px; max-width: 220px; }
  button { cursor: pointer; } button:disabled { opacity: .5; } label { display: flex; align-items: center; gap: 6px; font-size: 12px; }
  button:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  iframe { border: 0; width: 100%; flex: 1; min-height: 0; background: var(--bg-primary); }
  small { color: var(--text-muted); } [role="alert"] { color: var(--danger, #c44); overflow-wrap: anywhere; }
</style>
