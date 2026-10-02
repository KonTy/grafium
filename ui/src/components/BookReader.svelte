<script lang="ts">
  import { untrack } from "svelte";
  import { get } from "svelte/store";
  import { listen } from "@tauri-apps/api/event";
  import type { Page } from "../lib/api";
  import {
    bookOpen, bookReadBytes, bookSavePosition, bookNotesList, bookNoteChanges,
    bookSelection, bookJump, compatibleBookLocation, selectionForBook, hasBookNoteConflicts,
    type BookInfo, type BookLocation,
  } from "../lib/books";
  import { BOOK_FRAME_SANDBOX, readerFrameURL, readReaderMessage, type BookTocItem } from "../lib/bookReaderSecurity";
  import { showToast } from "../lib/toast.svelte";
  import type { StudyProgress } from "../lib/studies";
  import ReadingSurface from "./ReadingSurface.svelte";
  import ReaderNavigation from "./ReaderNavigation.svelte";
  import { readerFlow, readerTextSize, READER_TEXT_SIZES, setReaderTextSize } from "../lib/readerPreferences";
  import { bionicReaderEnabled, setBionicReaderEnabled } from "../lib/bionicReader";
  import { observeReaderTheme, readReaderTheme } from "../lib/bookReaderTheme";

  let { page, graphPath, onStudyProgress }: {
    page: Page; graphPath: string; onStudyProgress?: (progress: StudyProgress) => void;
  } = $props();
  let frame = $state<HTMLIFrameElement>();
  let surface = $state<ReadingSurface>();
  let url = $state("");
  let book = $state.raw<BookInfo | null>(null);
  let loading = $state(true);
  let ready = $state(false);
  let direction = $state<"ltr" | "rtl">("ltr");
  let error = $state("");
  let positionError = $state("");
  let invalidated = $state(false);
  let monitorError = $state("");
  let notice = $state("");
  let label = $state("");
  let toc = $state<BookTocItem[]>([]);
  let annotations = $state(false);
  let pdfPages = $state(0);
  let pdfPage = $state(1);
  let size = $state(100);
  let retry = $state(0);
  let sendCommand: (type: string, data?: Record<string, unknown>) => void = () => {};
  let openFrame: () => void = () => {};
  let saveNow: () => Promise<void> = async () => {};
  let verifySource: () => Promise<boolean> = async () => false;
  const selection = $derived(book ? selectionForBook($bookSelection, graphPath, book) : null);

  async function openNotes() {
    await surface?.exitFullscreen();
    if (book) window.dispatchEvent(new CustomEvent("book-open-notes", {
      detail: { graphPath, bookId: book.id, pageId: book.pageId },
    }));
  }

  $effect(() => {
    const graph = graphPath;
    const pageId = page.id;
    retry;
    let disposed = false;
    let activeBook: BookInfo | null = null;
    let pending: BookLocation | null = null;
    let positionTimer: ReturnType<typeof setTimeout> | undefined;
    let openTimer: ReturnType<typeof setTimeout> | undefined;
    let writing = Promise.resolve();
    const token = crypto.randomUUID();
    loading = true; ready = false; error = ""; positionError = ""; notice = ""; invalidated = false; monitorError = "";
    url = ""; book = null; toc = []; label = ""; annotations = false; pdfPages = 0; size = 100;
    let bytes: ArrayBuffer | null = null;
    let runtime = "";
    let checking: Promise<boolean> | null = null;
    let unlisten: (() => void) | undefined;
    function invalidate(message: string) {
      if (disposed) return;
      invalidated = true; ready = false; annotations = false; pending = null;
      clearTimeout(positionTimer);
      error = `The original book is no longer verified. ${message}`;
      notice = "Stale read-only snapshot: position saving and passage annotations are disabled. Reload the book after restoring or reimporting the source.";
      const selected = get(bookSelection);
      if (selected?.graphPath === graph && selected.pageId === pageId) bookSelection.set(null);
    }
    function validateSource(): Promise<boolean> {
      if (disposed || !activeBook || invalidated) return Promise.resolve(false);
      if (checking) return checking;
      const expected = activeBook;
      checking = bookOpen(graph, pageId).then(current => {
        if (disposed) return false;
        if (current.id !== expected.id || current.sourceSha256 !== expected.sourceSha256
          || current.format !== expected.format || current.filePath !== expected.filePath) {
          invalidate("The source was replaced or changed.");
          return false;
        }
        activeBook = current;
        book = current;
        return true;
      }).catch(e => { invalidate(String(e)); return false; }).finally(() => { checking = null; });
      return checking;
    }
    verifySource = validateSource;
    const revalidate = () => { if (!document.hidden) void validateSource(); };
    sendCommand = (type, data = {}) => {
      if (!disposed) frame?.contentWindow?.postMessage({ channel: "grafium-book", token, type, ...data }, "*");
    };
    const flush = () => {
      clearTimeout(positionTimer);
      if (!pending || !activeBook) return writing;
      const location = pending;
      const source = activeBook;
      pending = null;
      writing = writing.then(async () => {
        try {
          await bookSavePosition(graph, source.id, source.sourceSha256, location);
          if (!disposed) positionError = "";
        } catch (e) {
          if (disposed) showToast(`Book position was not saved: ${String(e)}`);
          else {
            pending ??= location; positionError = `Position not saved. ${String(e)}`;
            await validateSource();
          }
        }
      });
      return writing;
    };
    saveNow = flush;
    const receive = (event: MessageEvent) => {
      const message = readReaderMessage(event, frame?.contentWindow ?? null, token);
      if (!message || disposed || !activeBook) return;
      if (message.type === "help") {
        void surface?.exitFullscreen().then(() => frame?.dispatchEvent(new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true })));
      } else if (message.type === "toggle-controls") {
        surface?.toggleControls();
      } else if (message.type === "toggle-fullscreen") {
        void surface?.toggleFullscreen();
      } else if (message.type === "exit-fullscreen") {
        void surface?.dismiss();
      } else if (message.type === "toggle-bionic") {
        setBionicReaderEnabled(!get(bionicReaderEnabled));
      } else if (message.type === "bookmark") {
        showToast("Use Book notes for this imported book. Bookmarks are available in Library.", "error");
      } else if (message.type === "ready" && !invalidated) {
        clearTimeout(openTimer); loading = false; ready = true;
        sendCommand("theme", { theme: readReaderTheme() });
        toc = message.toc; notice = message.notice; annotations = message.annotations; pdfPages = message.pages ?? 0;
        direction = message.direction ?? "ltr";
      } else if (message.type === "error") {
        clearTimeout(openTimer); loading = false; error = message.message;
      } else if (message.type === "location" && !invalidated && compatibleBookLocation(activeBook, message.location)) {
        pending = message.location; label = message.label;
        if (message.location.kind === "pdf") pdfPage = message.location.page;
        onStudyProgress?.({
          position: message.location.kind === "pdf" ? message.location.page : (message.fraction ?? 0) * 100,
          total: message.location.kind === "pdf" ? pdfPages : message.fraction === undefined ? 0 : 100,
          anchor: "", label: message.label,
        });
        clearTimeout(positionTimer);
        positionTimer = setTimeout(() => { void flush(); }, 600);
      } else if (message.type === "selection" && !invalidated && annotations && compatibleBookLocation(activeBook, message.location)) {
        bookSelection.set({ graphPath: graph, bookId: activeBook.id, pageId, sourceSha256: activeBook.sourceSha256,
          quote: message.quote, locator: message.location });
      } else if (message.type === "open-notes") openNotes();
    };
    window.addEventListener("message", receive);
    const stopTheme = untrack(() => observeReaderTheme(theme => sendCommand("theme", { theme })));
    window.addEventListener("pagehide", flush);
    window.addEventListener("focus", revalidate);
    document.addEventListener("visibilitychange", revalidate);
    void listen<{ graphPath: string }>("book-source-changed", event => {
      if (event.payload?.graphPath === graph) void validateSource();
    }).then(stop => { if (disposed) stop(); else unlisten = stop; }).catch(e => {
      if (!disposed) monitorError = `Live source notifications unavailable; source checks still run when returning to the window or after a failed save. ${String(e)}`;
    });
    openFrame = () => {
      if (disposed || !bytes || !activeBook) return;
      sendCommand("bootstrap", { runtime });
      frame?.contentWindow?.postMessage({
        channel: "grafium-book", token, type: "open", bytes, format: activeBook.format, location: activeBook.readingLocation,
      }, "*", [bytes]);
      bytes = null;
    };
    void (async () => {
      try {
        const info = await bookOpen(graph, pageId);
        if (disposed) return;
        if (info.pageId !== pageId) throw new Error("Book source does not match the requested page.");
        activeBook = info; book = info;
        const [data, response] = await Promise.all([
          bookReadBytes(graph, info.id), fetch("/book-reader/runtime.js"),
        ]);
        if (!response.ok) throw new Error("Offline reader runtime is missing. Run npm run build:reader before starting Grafium.");
        runtime = await response.text();
        if (disposed) return;
        if (!(data instanceof ArrayBuffer)) throw new Error("The backend did not return binary book bytes.");
        bytes = data; url = readerFrameURL(token, readReaderTheme());
        openTimer = setTimeout(() => {
          if (!disposed) { loading = false; error = "The isolated reader did not initialize. This WebView may not support the required offline frame/worker APIs."; }
        }, 45000);
      } catch (e) {
        if (!disposed) { error = `Could not open this original book. ${String(e)}`; loading = false; }
      }
    })();
    return () => {
      disposed = true; void flush(); clearTimeout(openTimer);
      window.removeEventListener("message", receive);
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("focus", revalidate);
      document.removeEventListener("visibilitychange", revalidate);
      unlisten?.();
      stopTheme();
      const selected = get(bookSelection);
      if (selected?.graphPath === graph && selected.pageId === pageId) bookSelection.set(null);
    };
  });

  $effect(() => {
    const textSize = $readerTextSize;
    if (ready && annotations && !pdfPages) untrack(() => sendCommand("size", { value: textSize }));
  });
  $effect(() => {
    const flow = $readerFlow, enabled = $bionicReaderEnabled;
    if (ready) untrack(() => {
      sendCommand("flow", { value: flow });
      sendCommand("bionic", { enabled });
    });
  });
  $effect(() => {
    $bookNoteChanges;
    if (!ready || !book) return;
    const source = book;
    const graph = graphPath;
    let cancelled = false;
    void bookNotesList(graph, source.id).then(notes => {
      if (!cancelled) sendCommand("notes", { locations: notes.filter(n =>
        !hasBookNoteConflicts(n) && n.status === "attached" && n.sourceSha256 === source.sourceSha256 && n.locator
        && compatibleBookLocation(source, n.locator)).map(n => n.locator) });
    }).catch(e => {
      if (!cancelled) {
        error = `Could not refresh highlights. ${String(e)}`;
        void verifySource();
      }
    });
    return () => { cancelled = true; };
  });
  $effect(() => {
    const jump = $bookJump;
    if (ready && book && jump?.graphPath === graphPath && jump.bookId === book.id
      && jump.sourceSha256 === book.sourceSha256 && compatibleBookLocation(book, jump.locator)) {
      sendCommand("goto", { location: jump.locator });
      bookJump.set(null);
    }
  });
</script>

<section class="book-reader" aria-label="Original book reader" data-help-context="books" data-book-page-id={page.id}>
  <ReadingSurface bind:this={surface} onNavigate={ready ? direction => sendCommand("turn", { direction }) : undefined}>
    {#snippet navigation()}
      <ReaderNavigation {ready} {direction} reflowable={annotations && !pdfPages} onNavigate={direction => sendCommand("turn", { direction })} />
      {#if toc.length}
        <select aria-label="Book contents" disabled={!ready} value="" onchange={e => {
          const item = toc[Number(e.currentTarget.value)];
          if (item) sendCommand("toc", { target: item.target });
          e.currentTarget.value = "";
        }}>
          <option value="" disabled>Contents…</option>
          {#each toc as item, i}<option value={i}>{"　".repeat(item.depth)}{item.label}</option>{/each}
        </select>
      {/if}
      {#if pdfPages}
        <label>Page <input aria-label="PDF page" type="number" min="1" max={pdfPages} value={pdfPage}
          onchange={e => {
            const page = Number(e.currentTarget.value);
            if (Number.isSafeInteger(page) && page >= 1 && page <= pdfPages) sendCommand("goto", { location: { kind: "pdf", page } });
          }} /> / {pdfPages}</label>
      {/if}
      <label>{pdfPages ? "Zoom" : "Text size"}
        <select aria-label={pdfPages ? "PDF zoom" : "Book text size"} value={pdfPages ? size : $readerTextSize}
          title={pdfPages ? "PDF zoom" : "Book text size (remembered across books and restarts)"} disabled={!ready || (!pdfPages && !annotations)}
          onchange={event => {
            const value = Number(event.currentTarget.value);
            if (pdfPages) { size = value; sendCommand("size", { value }); }
            else setReaderTextSize(value);
          }}>
          {#each READER_TEXT_SIZES as value}<option {value}>{value}%</option>{/each}
        </select>
      </label>
      <button type="button" disabled={!book} onclick={openNotes}>{selection ? "Note selection" : "Book notes"}</button>
    {#if label}<p class="position" aria-live="polite">{label}</p>{/if}
    {/snippet}
    {#snippet children()}
    {#if notice}<p class="notice">{notice}</p>{/if}
    {#if book?.indexingWarning}<p class="notice">Text indexing: {book.indexingWarning}</p>{/if}
    {#if monitorError}<p class="notice" role="status">{monitorError}</p>{/if}
    {#if positionError}<div role="alert">{positionError} <button onclick={() => { void saveNow(); }}>Retry position save</button></div>{/if}
    {#if error}<div role="alert">{error} <button onclick={() => retry++}>Reload book</button></div>{/if}
  {#if loading}<p role="status" class="loading">Opening local original book…</p>{/if}
  {#if url}
    <iframe bind:this={frame} src={url} sandbox={BOOK_FRAME_SANDBOX} title="Isolated original book"
      referrerpolicy="no-referrer" onload={() => openFrame()}></iframe>
  {/if}
    {/snippet}
  </ReadingSurface>
</section>

<style>
  .book-reader { display:flex; flex-direction:column; flex:1; min-width:0; min-height:0; height:100%; color:var(--text-primary); background:var(--bg-primary); }
  label { display:flex; align-items:center; gap:5px; font-size:12px; }
  button,select,input { font:inherit; font-size:12px; color:var(--text-primary); background:var(--bg-tertiary); border:1px solid var(--border); border-radius:5px; padding:6px 8px; min-height:32px; }
  select { max-width:260px; }
  input { width:64px; }
  button { cursor:pointer; } button:disabled { opacity:.5; cursor:default; }
  button:focus-visible,select:focus-visible,input:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
  iframe { flex:1; min-height:0; width:100%; border:0; background:var(--bg-primary); }
  .notice,.position { color:var(--text-secondary); font-size:12px; margin:6px 0 0; }
  [role="alert"] { font-size:12px; padding:6px 0; color:var(--danger,var(--text-primary)); }
  .loading { padding:10px 18px; }
</style>
