<script lang="ts">
  import { get } from "svelte/store";
  import { getGraphInfo } from "../lib/api";
  import type { PageNavigationTarget } from "../lib/navigation";
  import { assetBaseDirFor, hydrateAssetMedia } from "../lib/markdown";
  import { readingNoteRelativePath, renderReadingNoteMarkdown } from "../lib/readingNotes";
  import { bookOpen, bookSelection, selectionForBook, jumpToBookNote, compatibleBookLocation, type BookNote } from "../lib/books";
  import {
    getBookNotesSource, newBookNoteDraft, editBookNote, useBookSelection, clearBookPassage,
    loadBookNotes, saveBookNoteDraft, deleteBookNote, reviewBookNoteRevision,
    bookDraftChanges, updateBookDrafts, bookDraftDirty, type BookNotesSource,
  } from "../lib/bookNoteDrafts";

  let { pageId, pageTitle, active = true, onNavigate = () => {} }: {
    pageId: string; pageTitle: string; active?: boolean; onNavigate?: (target: PageNavigationTarget) => void;
  } = $props();
  let source = $state.raw<BookNotesSource | null>(null);
  let error = $state("");
  let retry = $state(0);
  let pendingDelete = $state.raw<{ source: BookNotesSource; note: BookNote } | null>(null);
  let deleting = $state(false);
  const view = $derived.by(() => {
    $bookDraftChanges;
    if (!source) return null;
    const draft = source.activeId ? source.drafts.get(source.activeId) : null;
    return { ...source, notes: [...source.notes], drafts: [...source.drafts.values()],
      draft: draft ? { ...draft } : null, dirty: draft ? bookDraftDirty(draft) : false };
  });
  const selection = $derived(source ? selectionForBook($bookSelection, source.graphPath, source.book) : null);
  const conflict = $derived(view?.notes.find(n => n.id === view.draft?.id && n.revision !== view.draft.note?.revision));
  $effect(() => {
    const destination = source;
    if (!destination || !active) return;
    const refresh = (event: Event) => {
      const change = (event as CustomEvent<{ graphPath: string }>).detail;
      if (change?.graphPath === destination.graphPath) void loadBookNotes(destination);
    };
    window.addEventListener("graph-sources-changed", refresh);
    return () => window.removeEventListener("graph-sources-changed", refresh);
  });
  $effect(() => {
    const id = pageId;
    retry;
    if (!active || !id) return;
    let disposed = false;
    source = null; error = ""; pendingDelete = null;
    void (async () => {
      try {
        const graph = await getGraphInfo();
        const book = await bookOpen(graph.path, id);
        if (disposed) return;
        if (book.pageId !== id) throw new Error("Book source does not match this page.");
        const destination = getBookNotesSource(graph.path, book);
        source = destination;
        if (!destination.activeId) newBookNoteDraft(destination);
        await loadBookNotes(destination);
      } catch (e) { if (!disposed) error = `Could not open book notes. Drafts are kept. ${String(e)}`; }
    })();
    return () => { disposed = true; };
  });
  const draft = () => source?.activeId ? source.drafts.get(source.activeId) : null;
  function body(value: string) { const d = draft(); if (d && !d.saving) { d.body = value; d.notice = ""; updateBookDrafts(); } }
  function useSelection() { const d = draft(); if (source && d) useBookSelection(source, d, get(bookSelection)); }
  function clearPassage() { const d = draft(); if (source && d) clearBookPassage(source, d); }
  function save() { const d = draft(); if (source && d) void saveBookNoteDraft(source, d); }
  function newNote() { if (source) { const d = newBookNoteDraft(source); if (selection) useBookSelection(source, d, selection); } }
  function jump(note: BookNote) {
    if (!source) return;
    try {
      jumpToBookNote(source.graphPath, source.book, note);
      const reader = document.querySelector<HTMLElement>(".book-reader[data-book-page-id]");
      if (reader?.dataset.bookPageId !== source.book.pageId) onNavigate({ id: source.book.pageId });
    }
    catch (e) { error = String(e); }
  }
  function canJump(note: BookNote) {
    return !!source && note.status === "attached" && note.sourceSha256 === source.book.sourceSha256
      && !!note.locator && compatibleBookLocation(source.book, note.locator);
  }
  function review(revision: string) { const d = draft(); if (source && d) reviewBookNoteRevision(source, d, revision); }
  async function remove() {
    const request = pendingDelete;
    if (!request || deleting) return;
    deleting = true;
    try { await deleteBookNote(request.source, request.note); pendingDelete = null; }
    catch (e) { error = `Could not remove note. No draft was discarded. ${String(e)}`; }
    finally { deleting = false; }
  }
  function preview(note: BookNote, graphPath: string) {
    return renderReadingNoteMarkdown(note.body, assetBaseDirFor(readingNoteRelativePath(graphPath, note.filePath)));
  }
  function media(node: HTMLElement, _body: string) {
    let cleanup = hydrateAssetMedia(node);
    return { update(_value: string) { cleanup(); cleanup = hydrateAssetMedia(node); }, destroy() { cleanup(); } };
  }
</script>

<section class="book-notes-panel" aria-label="Original book notes" data-help-context="books">
  <header>
    <h2>{source?.book.title || pageTitle}</h2>
    <p>Notes are separate graph Markdown files. The original book is never rewritten.</p>
    <p>Drafts survive navigation and graph switching in this session, not closing the app. <strong>Save note before closing.</strong></p>
  </header>
  {#if error}<div class="error" role="alert">{error} <button onclick={() => retry++}>Retry</button></div>{/if}
  {#if !view}
    {#if !error}<p role="status">Loading book notes…</p>{/if}
  {:else}
    <div class="toolbar">
      <button onclick={newNote}>New note</button>
      <button disabled={view.loading} onclick={() => source && loadBookNotes(source)}>Refresh</button>
    </div>
    {#if view.error}<div class="error" role="alert">{view.error}</div>{/if}
    {#if view.drafts.length > 1}
      <label>Drafts kept for this book
        <select aria-label="Book note draft" value={view.activeId ?? ""} onchange={e => {
          if (source) { source.activeId = e.currentTarget.value; updateBookDrafts(); }
        }}>
          {#each view.drafts as d}
            <option value={d.id}>{bookDraftDirty(d) ? "Unsaved · " : d.note ? "Saved · " : "New · "}{d.body.trim().slice(0, 60) || "Untitled note"}</option>
          {/each}
        </select>
      </label>
    {/if}
    {#if view.draft}
      <div class="composer">
        <h3>{view.draft.note ? "Edit note" : "New book note"}</h3>
        {#if view.draft.sourceSha256 !== view.book.sourceSha256}
          <p class="error">Original file changed. This old passage is not attached. Existing note text can still be saved without changing its anchor. To reattach, choose a current selection or Whole-book note explicitly.</p>
        {/if}
        {#if view.draft.quote}<blockquote>{view.draft.quote}</blockquote>{:else}<p>Whole-book note · no passage attached</p>{/if}
        <div class="toolbar">
          <button disabled={!selection || view.draft.saving} onclick={useSelection}>Use selection</button>
          <button disabled={view.draft.saving} onclick={clearPassage}>Whole-book note</button>
        </div>
        {#if selection}<p>{selection.quote.length} selected characters available from this book.</p>{/if}
        <textarea aria-label="Book note" rows="7" value={view.draft.body} disabled={view.draft.saving}
          oninput={e => body(e.currentTarget.value)} placeholder="Write a Markdown note…"></textarea>
        <div class="toolbar">
          <button class="save" disabled={view.draft.saving || !view.draft.body.trim() || (!!view.draft.note && !view.dirty)} onclick={save}>
            {view.draft.saving ? "Saving…" : "Save note"}
          </button>
          <span>{view.dirty ? "Unsaved draft" : view.draft.note ? "Saved" : "New draft"}</span>
        </div>
        {#if view.draft.error}<div class="error" role="alert">{view.draft.error}</div>{/if}
        {#if view.draft.notice}<p role="status">{view.draft.notice}</p>{/if}
        {#if conflict}
          <details class="revision">
            <summary>Review changed saved version</summary>
            <div class="markdown-preview" use:media={conflict.body}>{@html preview(conflict, view.graphPath)}</div>
            <blockquote>{conflict.quote}</blockquote>
            <button disabled={view.draft.saving} onclick={() => review(conflict.revision)}>Use reviewed version for retry</button>
          </details>
        {/if}
      </div>
    {/if}
    <div class="saved-notes">
      <h3>Saved notes ({view.notes.length})</h3>
      {#if view.loading}<p role="status">Refreshing notes…</p>{/if}
      {#each view.notes as note (note.id)}
        <article class="book-note-card">
          <p class="status">{note.status === "orphaned" || note.sourceSha256 !== view.book.sourceSha256 ? "Orphaned · source changed" : note.locator && !canJump(note) ? "Passage unavailable · reader version changed" : note.locator ? "Passage note" : "Whole-book note"}</p>
          {#if note.quote}<blockquote>{note.quote}</blockquote>{/if}
          <div class="markdown-preview" use:media={note.body}>{@html preview(note, view.graphPath)}</div>
          <div class="toolbar">
            <button onclick={() => source && editBookNote(source, note)}>Edit note</button>
            <button disabled={!canJump(note)} onclick={() => jump(note)}>Return to passage</button>
            <button onclick={() => onNavigate({ id: note.notePageId })}>Open Markdown note</button>
            <button disabled={view.drafts.some(d => d.id === note.id && d.saving)} onclick={() => {
              if (source) pendingDelete = { source, note };
            }}>Remove</button>
          </div>
          <small>{note.filePath}</small>
        </article>
      {/each}
    </div>
  {/if}
  {#if pendingDelete}
    <div class="remove-confirm" role="alertdialog" aria-label="Remove book note" aria-modal="false" tabindex="-1">
      <p>Remove this saved note from graph Markdown? The original book will not change. Unsaved edits are kept as a new draft.</p>
      <button disabled={deleting} onclick={remove}>{deleting ? "Removing…" : "Remove saved note"}</button>
      <button disabled={deleting} onclick={() => pendingDelete = null}>Cancel</button>
    </div>
  {/if}
</section>

<style>
  .book-notes-panel { display:flex; flex-direction:column; flex:1; min-width:0; min-height:0; overflow-y:auto; overflow-x:hidden; gap:12px; padding:0 2px 16px; font-size:13px; line-height:1.5; color:var(--text-primary); }
  .book-notes-panel > * { flex-shrink:0; }
  h2 { font-size:16px; margin:0 0 6px; } h3 { font-size:14px; margin:0 0 8px; }
  p { margin:5px 0; } header p,small,.status { color:var(--text-secondary); font-size:12px; overflow-wrap:anywhere; }
  .toolbar { display:flex; flex-wrap:wrap; gap:6px; align-items:center; }
  button,textarea,select { font:inherit; border:1px solid var(--border); border-radius:5px; color:var(--text-primary); }
  button { background:var(--bg-tertiary); padding:6px 9px; min-height:32px; cursor:pointer; }
  button:disabled { opacity:.5; cursor:default; }
  button:hover:not(:disabled) { background:var(--bg-hover); }
  button:focus-visible,textarea:focus-visible,select:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
  textarea,select { background:var(--bg-primary); width:100%; box-sizing:border-box; min-width:0; padding:8px; }
  textarea { resize:vertical; margin:10px 0; }
  blockquote { margin:8px 0; padding:4px 10px; border-left:2px solid var(--border); white-space:pre-wrap; overflow-wrap:anywhere; color:var(--text-secondary); max-height:160px; overflow:auto; }
  .save { background:var(--btn-primary-bg,var(--accent)); color:var(--btn-primary-fg,var(--bg-primary)); }
  .error { color:var(--danger,var(--text-primary)); overflow-wrap:anywhere; }
  .saved-notes,.book-note-card { padding-top:12px; border-top:1px solid var(--border); }
  .book-note-card { padding-bottom:14px; }
  .markdown-preview { overflow-wrap:anywhere; margin:10px 0; }
  .markdown-preview :global(img) { max-width:100%; }
  .remove-confirm,.revision { padding:10px; border:1px solid var(--border); background:var(--bg-secondary); border-radius:5px; }
  .remove-confirm { position:sticky; bottom:0; }
</style>
