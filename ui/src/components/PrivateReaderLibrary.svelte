<script lang="ts">
  import { onMount } from "svelte";
  import { privateLibrary, privateLibraryError, refreshPrivateLibrary, bookmarkLabel } from "../lib/privateReader";
  let { onOpen, onSettings }: { onOpen: (bookId: string) => void; onSettings: () => void } = $props();
  let scanning = $state(false);
  let query = $state("");
  const books = $derived($privateLibrary.books.filter(book => book.title.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())));
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

<section class="private-library" data-help-context="reader" aria-label="Private book library">
  <header><div><p class="eyebrow">ON THIS DEVICE · NOT IN YOUR GRAPH</p><h2>Private library</h2></div><div class="actions"><button onclick={onSettings}>Library settings</button><button disabled={scanning} onclick={refresh}>{scanning ? "Scanning…" : "Rescan"}</button></div></header>
  <p class="privacy">Books, progress, and automatic bookmarks stay app-private: no graph pages, AI indexing, or graph sync.
    Notes you intentionally write in your journal with [[Book title]] are ordinary graph notes and follow your graph’s sharing settings.</p>
  {#if $privateLibraryError}<p class="error" role="alert">{$privateLibraryError}</p>{/if}
  {#if !$privateLibrary.libraryPath}<p>Choose an external local folder in <button class="text-button" onclick={onSettings}>Settings → Library location</button>. Originals stay in that folder.</p>
  {/if}
  {#if $privateLibrary.libraryPath || $privateLibrary.books.length}
    <label class="filter">Find a private book<input type="search" bind:value={query} placeholder="Title…" /></label>
    {#if !books.length}<p class="empty">{scanning ? "Discovering books…" : $privateLibrary.books.length ? "No matching private books." : "No books discovered yet. Add EPUBs, audiobook folders, or loose MP3 files to your library folder, then rescan."}</p>{/if}
    <ul>
      {#each books as book (book.id)}
        <li>
          <div class="book-main"><span class="kind">{book.kind === "audio" ? "AUDIO" : "EPUB"}</span><button class="book-title" onclick={() => onOpen(book.id)}>{book.title}</button><small>{book.available ? book.position ? bookmarkLabel(book, book.position) : "Not started" : "Source unavailable · history retained"}</small></div>
          <span class="count">{book.bookmarks.length} bookmarks</span>
          <button onclick={() => onOpen(book.id)}>{book.available ? "Open" : "History / relink"}</button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .private-library { padding: 22px; margin: 24px 0; background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 12px; }
  header, .actions, li { display: flex; align-items: center; gap: 10px; } header { justify-content: space-between; flex-wrap: wrap; }
  h2 { margin: 4px 0 0; font-size: 20px; } .eyebrow { margin: 0; font-size: 9px; letter-spacing: .12em; color: var(--accent); }
  .privacy, small, .empty, .count { color: var(--text-muted); font-size: 12px; line-height: 1.6; } .privacy { max-width: 85ch; }
  button, input { font: inherit; color: var(--text-primary); border: 1px solid var(--border); background: var(--bg-primary); border-radius: 6px; padding: 7px 10px; }
  button { cursor: pointer; } button:disabled { opacity: .5; } button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .filter { display: flex; gap: 10px; align-items: center; font-size: 12px; margin: 18px 0; } input { min-width: 0; }
  ul { list-style: none; padding: 0; margin: 0; } li { border-top: 1px solid var(--border); padding: 14px 0; }
  .book-main { display: flex; flex: 1; min-width: 0; flex-direction: column; align-items: start; gap: 5px; }
  .kind { font-size: 9px; letter-spacing: .1em; color: var(--accent); }
  .book-title, .text-button { padding: 0; border: 0; background: none; text-align: left; color: var(--accent); }
  .book-title { font-weight: 600; overflow-wrap: anywhere; } .error { color: var(--danger, #c44); overflow-wrap: anywhere; }
  @media (max-width: 500px) { .private-library { padding: 14px; } .count { display: none; } .filter { align-items: stretch; flex-direction: column; } }
</style>
