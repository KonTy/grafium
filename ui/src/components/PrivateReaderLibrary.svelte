<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { get } from "svelte/store";
  import { privateLibrary, privateLibraryError, refreshPrivateLibrary, setPrivateFavorite, addPrivateLibraryLink, type ReaderBook } from "../lib/privateReader";
  import { disconnectedLocations, libraryLocations, libraryLocationName, libraryLocationVolume, unavailableSourceMessage } from "../lib/libraryLocations";
  import { libraryBooks, libraryLink, libraryPercent, libraryPositionLabel, requestLibraryMedia } from "../lib/library";
  import { FALLBACK_LIBRARY_INDEX_STATUS, createStaleLibrarySearchGuard, formatLibrarySourcePosition, isLibraryIndexOffError, libraryIndexStatus, librarySearch, subscribeLibraryIndexUpdated, type LibraryIndexStatus, type LibrarySearchHit } from "../lib/libraryIndex";
  import { playPrivateAudio, privatePlayback, resumePrivatePlayback } from "../lib/privateReaderPlayback";
  import SettingsHelp from "./SettingsHelp.svelte";
  import { isAndroidReader } from "../lib/privateReaderAndroid";
  import { shortcutTitle, shortcutAria } from "../lib/shortcuts";
  import { showToast } from "../lib/toast.svelte";
  const TYPE_KEY = "grafium.library.mediaType";
  const MEDIA_TYPES = ["all", "epub", "audio", "video", "youtube"];
  let { onOpen, onSettings, onAddToStudies }: {
    onOpen: (bookId: string, position?: { trackId?: string | null; startMs?: number | null; quote?: string | null; chapter?: string | null }) => void; onSettings: () => void; onAddToStudies?: (book: ReaderBook) => void;
  } = $props();
  let scanning = $state(false);
  let query = $state("");
  let searchInput: HTMLInputElement | undefined = $state();
  let favorites = $state(false);
  let kind = $state(loadMediaType());
  let adding = $state(false);
  let link = $state("");
  let title = $state("");
  let linkKind = $state<"auto" | "audio" | "video">("auto");
  let busy = $state(false);
  let error = $state("");
  let message = $state("");
  let indexStatus = $state<LibraryIndexStatus | null>(null);
  let indexError = $state("");
  let contentHits = $state<LibrarySearchHit[]>([]);
  let contentSearching = $state(false);
  let contentError = $state("");
  const guardedSearch = createStaleLibrarySearchGuard(librarySearch);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let stopIndex: (() => void) | undefined;
  const books = $derived(libraryBooks($privateLibrary.books, query, favorites, kind));
  const configured = $derived(libraryLocations($privateLibrary).length > 0);
  const offline = $derived(disconnectedLocations($privateLibrary));
  const offlineNotice = $derived.by(() => {
    if (!offline.length) return "";
    const items = offline.reduce((sum, location) => sum + location.items, 0);
    const kept = `${items} ${items === 1 ? "item is" : "items are"} kept here`;
    return offline.length === 1
      ? `${libraryLocationName(offline[0].path)} isn't connected. Its ${kept} until you reconnect it.`
      : `${offline.length} locations aren't connected (${offline.map(location => libraryLocationName(location.path)).join(", ")}). Their ${kept} until you reconnect them.`;
  });
  const bookById = $derived(new Map($privateLibrary.books.map(book => [book.id, book])));
  function placeOf(book: ReaderBook): string {
    return book.location ? libraryLocationVolume(book.location) ?? libraryLocationName(book.location) : "its location";
  }

  const indexLine = $derived.by(() => {
    const status = indexStatus;
    if (!status || !(configured || $privateLibrary.books.length)) return "";
    const parts: string[] = [];
    if (status.running || status.items.pending || status.items.failed || status.items.titleOnly || status.items.waiting || status.semantic !== "ready" || status.transcription !== "ready") {
      parts.push(`Indexed ${status.items.indexed} of ${status.items.total} items`);
      if (status.running) parts.push("indexing now");
      if (status.items.pending) parts.push(`${status.items.pending} pending`);
      if (status.items.waiting) parts.push(`${status.items.waiting} waiting for a disconnected location`);
      if (status.items.failed) parts.push(`${status.items.failed} failed`);
      if (status.items.titleOnly) parts.push(`${status.items.titleOnly} title-only`);
      if (status.semantic !== "ready") parts.push(status.semanticReason || `semantic ${status.semantic}`);
      if (status.transcription !== "ready") parts.push(status.transcriptionReason || `transcription ${status.transcription}`);
    }
    return parts.join(" · ");
  });

  $effect(() => {
    const text = query.trim();
    const enabled = indexStatus?.enabled !== false;
    clearTimeout(searchTimer);
    guardedSearch.cancel();
    contentError = "";
    if (!text || !enabled) { contentHits = []; contentSearching = false; return; }
    contentSearching = true;
    searchTimer = setTimeout(() => {
      void guardedSearch(text, 8).then(results => {
        if (results === null) return;
        contentHits = results;
        contentSearching = false;
      }).catch(cause => {
        if (isLibraryIndexOffError(cause)) {
          indexStatus = { ...FALLBACK_LIBRARY_INDEX_STATUS, enabled: false, semanticReason: "Library index is off." };
          contentError = "";
        } else contentError = String(cause);
        contentHits = []; contentSearching = false;
      });
    }, 250);
    return () => { clearTimeout(searchTimer); guardedSearch.cancel(); };
  });

  function openHit(hit: LibrarySearchHit) {
    onOpen(hit.bookId, { trackId: hit.trackId, startMs: hit.startMs, quote: hit.quote, chapter: hit.chapter });
  }
  function loadMediaType(): string {
    try {
      const value = window.localStorage.getItem(TYPE_KEY);
      if (value === null) return "all";
      if (MEDIA_TYPES.includes(value)) return value;
      console.warn("Invalid saved Library type filter; showing all types.");
    } catch (cause) {
      console.warn("Could not load the Library type filter; showing all types.", cause);
    }
    return "all";
  }

  function setMediaType(value: string): void {
    if (!MEDIA_TYPES.includes(value)) return;
    kind = value;
    try { window.localStorage.setItem(TYPE_KEY, value); }
    catch (cause) {
      showToast(`Could not remember the Library type filter; it may reset when you leave Library or restart: ${String(cause)}`, "error");
    }
  }

  export function focusSearch(): boolean {
    if (!searchInput || searchInput.disabled) return false;
    searchInput.focus();
    searchInput.select();
    return true;
  }

  async function run(action: () => Promise<unknown>) {
    busy = true; error = ""; message = "";
    try { await action(); } catch (cause) { error = String(cause); } finally { busy = false; }
  }
  async function resume(book: ReaderBook) {
    if (book.kind !== "audio" || (book.sourceUrl && isAndroidReader())) {
      if (book.kind !== "epub") requestLibraryMedia(book.id);
      onOpen(book.id); return;
    }
    if ($privatePlayback.bookId === book.id && $privatePlayback.status === "playing") return;
    if ($privatePlayback.bookId === book.id && $privatePlayback.status === "paused" && !$privatePlayback.error)
      await resumePrivatePlayback();
    else await playPrivateAudio(book);
  }
  // A drive may have been plugged in since the last scan: check again first.
  async function openWhenConnected(book: ReaderBook) {
    scanning = true;
    try { await refreshPrivateLibrary(true); } catch { /* The shared store exposes the native failure. */ }
    finally { scanning = false; }
    const current = get(privateLibrary).books.find(item => item.id === book.id) ?? book;
    if (current.available) await resume(current);
    else showToast(unavailableSourceMessage(current), "info");
  }
  async function refresh() {
    scanning = true;
    try { await refreshPrivateLibrary(true); } catch { /* The shared store exposes the native failure. */ }
    finally { scanning = false; }
  }
  onMount(() => {
    void refresh();
    void libraryIndexStatus().then(value => { indexStatus = value; }).catch(cause => { indexError = String(cause); });
    void subscribeLibraryIndexUpdated(value => { indexStatus = value; }, message => { indexError = message; }).then(unlisten => { stopIndex = unlisten; }).catch(cause => { indexError = String(cause); });
    const focus = () => { if (!document.hidden) void refresh(); };
    window.addEventListener("focus", focus);
    return () => { window.removeEventListener("focus", focus); stopIndex?.(); };
  });
  onDestroy(() => { clearTimeout(searchTimer); stopIndex?.(); });
</script>

<section class="private-library" data-help-context="library" aria-label="Library">
  <header><div><p class="eyebrow">ON THIS DEVICE</p><h1>Library</h1></div><div class="actions">
    <SettingsHelp title="Your Library"><p>Books, media, progress, favorites, and bookmarks stay app-private: no graph pages, AI indexing, or graph sync. Actual reading and playback move an item to the top; opening its details does not.</p><p>Use Add to Studies to reference a source without copying its media or progress. A bookmark’s Journal note action opens a draft for your review; only saving that draft writes ordinary graph content.</p><p>Local files stay in the folder chosen in Settings. Add EPUBs, audio, or videos there, then rescan. Network links connect only when played. Video and YouTube stop when you leave their player. Audio and read aloud use the persistent toolbar, except network audio on Android, which stops when you leave its player. Local video currently requires desktop.</p></SettingsHelp>
    <button aria-expanded={adding} onclick={() => adding = !adding}>Add link</button><button onclick={onSettings}>Library settings</button><button disabled={scanning} onclick={refresh}>{scanning ? "Scanning…" : "Rescan"}</button></div></header>
  {#if adding}
    <form class="link-form" onsubmit={event => { event.preventDefault(); void run(async () => {
      const source = libraryLink(link, title, linkKind);
      await addPrivateLibraryLink(source.title, source.kind, source.url);
      adding = false; link = ""; title = ""; linkKind = "auto"; message = "Link added to Library.";
    }); }}>
      <label>Media URL<input type="url" required bind:value={link} placeholder="https://…" /></label>
      <label>Title (optional)<input bind:value={title} maxlength="512" /></label>
      <label>Type<select bind:value={linkKind}><option value="auto">Detect from link</option><option value="audio">Direct audio</option><option value="video">Direct video</option></select></label>
      <div class="actions"><button disabled={busy}>Add to Library</button><button type="button" onclick={() => adding = false}>Cancel</button></div>
    </form>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
  {#if $privateLibraryError}<p class="error" role="alert">{$privateLibraryError}</p>{/if}
  {#if indexError}<p class="error compact" role="alert">Library index status unavailable: {indexError}</p>{/if}
  {#if indexLine}<p class="index-status" role="status">{indexLine}</p>{/if}
  {#if offlineNotice}<p class="offline-notice" role="status">{offlineNotice} <button class="text-button" disabled={scanning} onclick={refresh}>{scanning ? "Checking…" : "Check again"}</button></p>{/if}
  {#if !configured}<p>Add a folder, drive, SD card, or mounted share in <button class="text-button" onclick={onSettings}>Settings → Library</button>. Originals stay where they are.</p>
  {/if}
  {#if configured || $privateLibrary.books.length}
    <div class="filters"><label class="filter">Search Library<input type="search" bind:this={searchInput} bind:value={query}
      data-local-search placeholder="Search titles…" title={shortcutTitle("Search Library", "search-local")}
      aria-keyshortcuts={shortcutAria("search-local")} /></label>
      <label>Type<select value={kind} onchange={event => setMediaType(event.currentTarget.value)}><option value="all">All types</option><option value="epub">EPUB</option><option value="audio">Audio</option><option value="video">Video</option><option value="youtube">YouTube</option></select></label>
      <button aria-pressed={favorites} onclick={() => favorites = !favorites}>★ Favorites</button>
    </div>
    {#if query.trim()}
      <section class="inside-results" aria-label="Inside your Library">
        <h2>Inside your Library</h2>
        {#if indexStatus?.enabled === false}<p class="empty">Inside search is off in Settings → Library.</p>{/if}
        {#if contentSearching}<p class="empty" role="status">Searching indexed content…</p>{/if}
        {#if contentError}<p class="error compact" role="alert">Inside search failed: {contentError}</p>{/if}
        {#if indexStatus?.enabled !== false && !contentSearching && !contentError && !contentHits.length}<p class="empty">No inside matches yet.</p>{/if}
        {#if contentHits.length}
          <ul class="hit-list">{#each contentHits as hit, index (hit.chunkId ?? `${index}:${hit.bookId}:${hit.trackId ?? ""}:${hit.startMs ?? ""}:${hit.quote ?? hit.snippet}`)}
            <li>
              <button class="hit" onclick={() => openHit(hit)}>
                <span><strong>{hit.title}</strong> <small>{hit.kind.toUpperCase()} · {formatLibrarySourcePosition(hit)}{bookById.get(hit.bookId)?.disconnected ? " · Disconnected" : ""}</small></span>
                <span class="snippet">{hit.snippet}</span>
              </button>
            </li>
          {/each}</ul>
        {/if}
      </section>
    {/if}
    <p class="empty">Recently read or played</p>
    {#if !books.length}<p class="empty">{scanning ? "Discovering sources…" : $privateLibrary.books.length ? "No matching Library items." : "No sources discovered yet. Add files to your library folder and rescan, or add a media link."}</p>{/if}
    <ul>
      {#each books as book (book.id)}
        <li>
          <div class="book-main"><span class="kind">{book.kind.toUpperCase()}</span><button class="book-title" onclick={() => onOpen(book.id)}>{book.title}</button>
            <small>{libraryPositionLabel(book)}{libraryPercent(book) !== null ? ` · ${libraryPercent(book)}%` : ""}</small>
            {#if libraryPercent(book) !== null}<progress max="100" value={libraryPercent(book) ?? 0} aria-label={`${book.title} progress`}></progress>{/if}
            {#if book.disconnected}<small class="offline">Disconnected · {placeOf(book)}</small>
            {:else if !book.available}<small>Source unavailable · history retained</small>{/if}
          </div>
          <span class="count">{book.bookmarks.length} bookmarks</span>
          <div class="actions row-actions">
            <button disabled={busy} aria-label={`${book.favorite ? "Unfavorite" : "Favorite"} ${book.title}`} aria-pressed={book.favorite ?? false} onclick={() => run(() => setPrivateFavorite(book.id, !book.favorite))}>{book.favorite ? "★" : "☆"}</button>
            {#if onAddToStudies}<button onclick={() => onAddToStudies?.(book)}>Add to Studies</button>{/if}
            {#if book.available}<button disabled={busy} onclick={() => run(() => resume(book))}>{book.kind === "epub" ? "Read" : book.position ? "Resume" : "Play"}</button>
            {:else if book.disconnected}<button disabled={busy || scanning} title={`On ${placeOf(book)}, which isn't connected`} onclick={() => run(() => openWhenConnected(book))}>{book.kind === "epub" ? "Read" : book.position ? "Resume" : "Play"}</button>
            {:else}<button onclick={() => onOpen(book.id)}>History / relink</button>{/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .private-library { padding: 22px; margin: 24px 0; background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 12px; }
  header, .actions, li { display: flex; align-items: center; gap: 10px; } header { justify-content: space-between; flex-wrap: wrap; }
  h1 { margin: 4px 0 0; font-size: 26px; } .eyebrow { margin: 0; font-size: 9px; letter-spacing: .12em; color: var(--accent); }
  small, .empty, .count { color: var(--text-muted); font-size: 12px; line-height: 1.6; }
  button, input, select { font: inherit; color: var(--text-primary); border: 1px solid var(--border); background: var(--bg-primary); border-radius: 6px; padding: 7px 10px; }
  button { cursor: pointer; } button:disabled { opacity: .5; } button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button[aria-expanded="true"] { color: var(--accent); border-color: var(--accent); }
  .filter { display: flex; flex: 1 1 360px; min-width: 0; gap: 10px; align-items: center; font-size: 12px; margin: 18px 0; } input { min-width: 0; }
  .filter input { flex: 1; width: 100%; min-height: 40px; font-size: 15px; }
  .filters, .row-actions { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; }
  .link-form { display: flex; flex-direction: column; gap: 12px; margin: 20px 0; }
  .link-form label { display: flex; flex-direction: column; gap: 5px; }
  progress { width: min(200px, 100%); height: 5px; accent-color: var(--accent); }
  ul { list-style: none; padding: 0; margin: 0; } li { border-top: 1px solid var(--border); padding: 14px 0; }
  .book-main { display: flex; flex: 1; min-width: 0; flex-direction: column; align-items: start; gap: 5px; }
  .kind { font-size: 9px; letter-spacing: .1em; color: var(--accent); }
  .book-title, .text-button { padding: 0; border: 0; background: none; text-align: left; color: var(--accent); }
  .book-title { font-weight: 600; overflow-wrap: anywhere; } .error { color: var(--danger, #c44); overflow-wrap: anywhere; } .compact { font-size: 12px; }
  .index-status { margin: 10px 0; color: var(--text-muted); font-size: 12px; }
  .offline-notice { margin: 12px 0; padding: 8px 10px; border: 1px solid var(--border); border-radius: 8px; font-size: 13px; }
  .offline { color: var(--task-todo-fg, var(--text-secondary)); } .inside-results { margin: 12px 0 18px; padding: 10px; border: 1px solid var(--border); border-radius: 8px; background: var(--bg-primary); }
  .inside-results h2 { margin: 0 0 8px; font-size: 15px; } .hit-list li { border-top: 1px solid var(--border); padding: 8px 0; } .hit-list li:first-child { border-top: 0; }
  .hit { display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%; border: 0; background: none; text-align: left; } .snippet { color: var(--text-primary); font-size: 13px; }
  @media (max-width: 500px) { .private-library { padding: 14px; } .count { display: none; } .filter { flex-basis: 100%; align-items: stretch; flex-direction: column; margin-bottom: 0; } li { flex-wrap: wrap; } .book-main { flex-basis: 100%; } }
</style>
