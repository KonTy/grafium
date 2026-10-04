<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import ReadingSurface from "./ReadingSurface.svelte";
  import ReaderNavigation from "./ReaderNavigation.svelte";
  import { readerFlow, readerTextSize, READER_TEXT_SIZES, setReaderTextSize } from "../lib/readerPreferences";
  import { bionicReaderEnabled, setBionicReaderEnabled } from "../lib/bionicReader";
  import { observeReaderTheme, readReaderTheme } from "../lib/bookReaderTheme";
  import { get } from "svelte/store";
  import { BOOK_FRAME_SANDBOX, readReaderMessage, readerFrameURL, type BookTocItem } from "../lib/bookReaderSecurity";
  import { privateBookJump, privateLibrary, privateVisualPositions, readerNative, privateLibraryError, refreshPrivateLibrary, privateBookLanguages, privateVoiceLanguageSuggestion } from "../lib/privateReader";
  import type { BookLocation } from "../lib/bookLocations";
  import type { ReaderTextSegment, ReaderMessage, ReaderBookmarkCapture } from "../lib/bookReaderSecurity";
  import { registerPrivateSegments } from "../lib/privateReaderSegments";
  import { privatePlayback } from "../lib/privateReaderPlayback";
  import { sha256 } from "@noble/hashes/sha256";
  import { saveLibraryCheckpoint, type LibraryProgress } from "../lib/library";
  import { findQuoteSegment, normalizeLibraryQuote, type QuoteSegment } from "../lib/privateBookQuoteMatch";
  import { onShortcutsChanged, shortcutBindings } from "../lib/shortcutRegistry";
  import { unavailableSourceMessage } from "../lib/libraryLocations";
  let { bookId, onActivity, onProgress, actions, bookmarks, bookmarkCount = 0, status, onBack, onBookmark }: {
    bookId: string; onActivity?: () => void; onProgress?: (progress: LibraryProgress) => void;
    actions?: Snippet; bookmarks?: Snippet; status?: Snippet; onBack?: () => void; onBookmark?: () => void;
    bookmarkCount?: number;
  } = $props();
  let surface = $state<ReadingSurface>();
  let frame = $state<HTMLIFrameElement>();
  let url = $state("");
  let ready = $state(false);
  let reflowable = $state(false);
  let direction = $state<"ltr" | "rtl">("ltr");
  let error = $state("");
  let label = $state("");
  let fraction = $state<number | undefined>();
  let toc = $state<BookTocItem[]>([]);
  let retry = $state(0);
  let send: (type: string, data?: Record<string, unknown>) => void = () => {};
  let bootstrap = () => {};
  let capture: () => Promise<ReaderBookmarkCapture> = async () => { throw new Error("Wait for the book to open before bookmarking."); };
  const segmentRequests = new Map<string, { resolve: (message: Extract<ReaderMessage, { type: "read-aloud-segments" }>) => void; reject: (cause: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  export function captureBookmark() { return capture(); }
  function requestReaderSegments(section: number, offset: number): Promise<Extract<ReaderMessage, { type: "read-aloud-segments" }>> {
    if (!ready) return Promise.reject(new Error("Wait for the book to open before searching it."));
    const requestId = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { segmentRequests.delete(requestId); reject(new Error("Book search timed out.")); }, 30000);
      segmentRequests.set(requestId, { resolve, reject, timer });
      send("read-aloud-segments", { requestId, section, offset });
    });
  }
  let citationNavigationPending = false;
  let citationNavigationTimer: ReturnType<typeof setTimeout> | undefined;
  function citationGoto(type: "goto" | "toc", data: Record<string, unknown>) {
    clearTimeout(citationNavigationTimer);
    citationNavigationPending = true;
    citationNavigationTimer = setTimeout(() => { citationNavigationPending = false; }, 5000);
    send(type, data);
  }
  export async function locateQuote(quote: string | null | undefined, chapter: string | null | undefined): Promise<"quote" | "chapter" | "start"> {
    if (!ready) throw new Error("Wait for the book to open before searching it.");
    const sections: QuoteSegment[][] = [];
    let section = 0; let offset = 0; let sectionSegments: QuoteSegment[] = [];
    while (quote && normalizeLibraryQuote(quote).length >= 40) {
      let response: Extract<ReaderMessage, { type: "read-aloud-segments" }>;
      try { response = await requestReaderSegments(section, offset); }
      catch { break; }
      sectionSegments.push(...response.segments.filter((segment): segment is QuoteSegment => segment.locator.kind === "epub"));
      if (response.nextOffset !== null) offset = response.nextOffset;
      else {
        sections.push(sectionSegments);
        sectionSegments = [];
        if (++section < response.sectionCount) offset = 0;
        else break;
      }
    }
    if (sectionSegments.length) sections.push(sectionSegments);
    const found = findQuoteSegment(quote, sections);
    if (found) {
      citationGoto("goto", { location: found.locator, select: true });
      return "quote";
    }
    const chapterNeedle = normalizeLibraryQuote(chapter ?? "");
    const chapterItem = chapterNeedle ? toc.find(item => normalizeLibraryQuote(item.label).includes(chapterNeedle)) : null;
    if (chapterItem) {
      citationGoto("toc", { target: chapterItem.target });
      return "chapter";
    }
    if (toc[0]) citationGoto("toc", { target: toc[0].target });
    return "start";
  }

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
    const bookmarkRequests = new Map<string, { resolve: (capture: ReaderBookmarkCapture) => void; reject: (cause: Error) => void; timer: ReturnType<typeof setTimeout> }>();
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
      if (["prev", "next", "turn", "toc", "goto"].includes(type)) navigationPending = true;
      else if (["size", "flow", "bionic", "theme"].includes(type)) navigationPending = false;
      if (!disposed) frame?.contentWindow?.postMessage({ channel: "grafium-book", token, type, ...data }, "*");
    };
    capture = () => {
      if (disposed || !ready || !sourceAvailable) return Promise.reject(new Error("Open an available book before bookmarking."));
      const requestId = crypto.randomUUID();
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          bookmarkRequests.delete(requestId);
          reject(new Error("The reader did not confirm the bookmark location. Nothing was saved."));
        }, 10000);
        bookmarkRequests.set(requestId, { resolve, reject, timer });
        send("capture-bookmark", { requestId });
      });
    };
    const rejectBookmarks = (cause: Error) => {
      for (const request of bookmarkRequests.values()) { clearTimeout(request.timer); request.reject(cause); }
      bookmarkRequests.clear();
    };
    const unregisterSegments = registerPrivateSegments(id, async () => {
      if (!ready) throw new Error("Wait for the isolated EPUB reader to finish loading.");
      const segments: ReaderTextSegment[] = [];
      let section = 0; let offset = 0; let size = 0;
      while (!disposed) {
        const response = await requestReaderSegments(section, offset);
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
      sendReaderShortcuts();
      frame?.contentWindow?.postMessage({ channel: "grafium-book", token, type: "open",
        bytes, format: "epub", location: book?.position?.locator }, "*", [bytes]);
      bytes = null;
    };
    const receive = (event: MessageEvent) => {
      const message = readReaderMessage(event, frame?.contentWindow ?? null, token);
      if (!message || disposed) return;
      if (message.type === "bookmark-captured") {
        const request = bookmarkRequests.get(message.requestId);
        if (!request) return;
        clearTimeout(request.timer); bookmarkRequests.delete(message.requestId);
        if (!sourceAvailable || message.location.kind !== "epub") request.reject(new Error("The bookmark source is unavailable or invalid."));
        else request.resolve({ location: message.location, quote: message.quote });
        return;
      }
      if (message.type === "read-aloud-segments") {
        const request = segmentRequests.get(message.requestId);
        if (request) { clearTimeout(request.timer); segmentRequests.delete(message.requestId); request.resolve(message); }
      }
      if (message.type === "ready") {
        clearTimeout(openTimer); ready = true; toc = message.toc;
        reflowable = message.annotations; direction = message.direction ?? "ltr";
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
        for (const request of segmentRequests.values()) { clearTimeout(request.timer); request.reject(new Error(message.message)); }
        segmentRequests.clear();
        rejectBookmarks(new Error(message.message));
      }
      else if (message.type === "help") void surface?.exitFullscreen().then(() =>
        frame?.dispatchEvent(new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true })));
      else if (message.type === "toggle-controls") surface?.toggleControls();
      else if (message.type === "toggle-fullscreen") void surface?.toggleFullscreen();
      else if (message.type === "exit-fullscreen") void surface?.dismiss();
      else if (message.type === "bookmark") surface?.bookmark();
      else if (message.type === "toggle-bionic") setBionicReaderEnabled(!get(bionicReaderEnabled));
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
        if (citationNavigationPending) {
          citationNavigationPending = false;
          clearTimeout(citationNavigationTimer);
          return;
        }
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
        const current = get(privateLibrary).books.find(item => item.id === id);
        if (!current?.available) {
          sourceAvailable = false; pending = null; ready = false;
          error = current?.disconnected
            ? `${unavailableSourceMessage(current)} The displayed EPUB is a read-only snapshot until then.`
            : "This source is no longer available. The displayed EPUB is a read-only snapshot; relink or restore the source, then reload.";
        }
      }).catch(cause => { if (!disposed) error = `Could not verify the private EPUB source: ${String(cause)}`; });
    };
    window.addEventListener("focus", checkSource);
    void (async () => {
      try {
        if (book?.disconnected) throw new Error(unavailableSourceMessage(book, "read"));
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
      disposed = true; void flush(); clearTimeout(openTimer); clearTimeout(citationNavigationTimer);
      unregisterSegments();
      for (const request of segmentRequests.values()) { clearTimeout(request.timer); request.reject(new Error("Private reader closed during narration preparation.")); }
      segmentRequests.clear();
      rejectBookmarks(new Error("The reader closed before its bookmark was captured."));
      privateVisualPositions.delete(id);
      window.removeEventListener("message", receive); window.removeEventListener("pagehide", flush);
      window.removeEventListener("focus", checkSource);
      stopTheme();
    };
  });
  // The isolated reader handles its own keys, so give it the current
  // Bionic and bookmark shortcuts from Settings.
  function sendReaderShortcuts() {
    send("shortcuts", {
      bindings: { "toggle-bionic": shortcutBindings("toggle-bionic"), bookmark: shortcutBindings("bookmark") },
    });
  }
  $effect(() => onShortcutsChanged(sendReaderShortcuts));
  $effect(() => {
    const size = $readerTextSize;
    if (ready && reflowable) untrack(() => send("size", { value: size }));
  });
  $effect(() => {
    const flow = $readerFlow, enabled = $bionicReaderEnabled;
    if (ready) untrack(() => {
      send("flow", { value: flow });
      send("bionic", { enabled });
    });
  });
  $effect(() => {
    const jump = $privateBookJump;
    if (ready && jump?.bookId === bookId) {
      send("goto", { location: jump.locator, select: jump.select === true });
      privateBookJump.set(null);
    }
  });
</script>

<section class="private-book" data-help-context="reader" aria-label="Private EPUB reader">
  <ReadingSurface bind:this={surface} {actions} {bookmarks} {bookmarkCount} {onBack} {onBookmark}
    onNavigate={ready ? direction => send("turn", { direction }) : undefined}>
  {#snippet navigation()}
    <ReaderNavigation {ready} {reflowable} {direction} onNavigate={direction => send("turn", { direction })} />
    <select disabled={!ready} aria-label="Private book contents" value="" onchange={event => {
      const item = toc[Number(event.currentTarget.value)];
      if (item) send("toc", { target: item.target });
      event.currentTarget.value = "";
    }}><option disabled value="">Contents…</option>{#each toc as item, index}<option value={index}>{"—".repeat(item.depth)} {item.label}</option>{/each}</select>
    <label>Text size<select aria-label="Book text size" title="Book text size (remembered across books and restarts)" value={$readerTextSize} disabled={!ready || !reflowable} onchange={event => { onActivity?.(); setReaderTextSize(Number(event.currentTarget.value)); }}>{#each READER_TEXT_SIZES as value}<option {value}>{value}%</option>{/each}</select></label>
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
