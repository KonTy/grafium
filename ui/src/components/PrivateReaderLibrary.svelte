<script lang="ts">
  import { onMount } from "svelte";
  import { privateLibrary, privateLibraryError, refreshPrivateLibrary, setPrivateFavorite, addPrivateLibraryLink, type ReaderBook } from "../lib/privateReader";
  import { libraryBooks, libraryLink, libraryPercent, libraryPositionLabel, requestLibraryMedia } from "../lib/library";
  import { playPrivateAudio, privatePlayback, resumePrivatePlayback } from "../lib/privateReaderPlayback";
  import SettingsHelp from "./SettingsHelp.svelte";
  import { isAndroidReader } from "../lib/privateReaderAndroid";
  import { shortcutTitle, shortcutAria } from "../lib/shortcuts";
  import { showToast } from "../lib/toast.svelte";
  const TYPE_KEY = "grafium.library.mediaType";
  const MEDIA_TYPES = ["all", "epub", "audio", "video", "youtube"];
  let { onOpen, onSettings, onAddToStudies }: {
    onOpen: (bookId: string) => void; onSettings: () => void; onAddToStudies?: (book: ReaderBook) => void;
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
  const books = $derived(libraryBooks($privateLibrary.books, query, favorites, kind));
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
  async function refresh() {
    scanning = true;
    try { await refreshPrivateLibrary(true); } catch { /* The shared store exposes the native failure. */ }
    finally { scanning = false; }
  }
  onMount(() => {
    void refresh();
    const focus = () => { if (!document.hidden) void refresh(); };
    window.addEventListener("focus", focus);
    return () => window.removeEventListener("focus", focus);
  });
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
  {#if !$privateLibrary.libraryPath}<p>Choose an external local folder in <button class="text-button" onclick={onSettings}>Settings → Library location</button>. Originals stay in that folder.</p>
  {/if}
  {#if $privateLibrary.libraryPath || $privateLibrary.books.length}
    <div class="filters"><label class="filter">Search Library<input type="search" bind:this={searchInput} bind:value={query}
      data-local-search placeholder="Search titles…" title={shortcutTitle("Search Library", "search-local")}
      aria-keyshortcuts={shortcutAria("search-local")} /></label>
      <label>Type<select value={kind} onchange={event => setMediaType(event.currentTarget.value)}><option value="all">All types</option><option value="epub">EPUB</option><option value="audio">Audio</option><option value="video">Video</option><option value="youtube">YouTube</option></select></label>
      <button aria-pressed={favorites} onclick={() => favorites = !favorites}>★ Favorites</button>
    </div>
    <p class="empty">Recently read or played</p>
    {#if !books.length}<p class="empty">{scanning ? "Discovering sources…" : $privateLibrary.books.length ? "No matching Library items." : "No sources discovered yet. Add files to your library folder and rescan, or add a media link."}</p>{/if}
    <ul>
      {#each books as book (book.id)}
        <li>
          <div class="book-main"><span class="kind">{book.kind.toUpperCase()}</span><button class="book-title" onclick={() => onOpen(book.id)}>{book.title}</button>
            <small>{libraryPositionLabel(book)}{libraryPercent(book) !== null ? ` · ${libraryPercent(book)}%` : ""}</small>
            {#if libraryPercent(book) !== null}<progress max="100" value={libraryPercent(book) ?? 0} aria-label={`${book.title} progress`}></progress>{/if}
            {#if !book.available}<small>Source unavailable · history retained</small>{/if}
          </div>
          <span class="count">{book.bookmarks.length} bookmarks</span>
          <div class="actions row-actions">
            <button disabled={busy} aria-label={`${book.favorite ? "Unfavorite" : "Favorite"} ${book.title}`} aria-pressed={book.favorite ?? false} onclick={() => run(() => setPrivateFavorite(book.id, !book.favorite))}>{book.favorite ? "★" : "☆"}</button>
            {#if onAddToStudies}<button onclick={() => onAddToStudies?.(book)}>Add to Studies</button>{/if}
            {#if book.available}<button disabled={busy} onclick={() => run(() => resume(book))}>{book.kind === "epub" ? "Read" : book.position ? "Resume" : "Play"}</button>
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
  .book-title { font-weight: 600; overflow-wrap: anywhere; } .error { color: var(--danger, #c44); overflow-wrap: anywhere; }
  @media (max-width: 500px) { .private-library { padding: 14px; } .count { display: none; } .filter { flex-basis: 100%; align-items: stretch; flex-direction: column; margin-bottom: 0; } li { flex-wrap: wrap; } .book-main { flex-basis: 100%; } }
</style>
