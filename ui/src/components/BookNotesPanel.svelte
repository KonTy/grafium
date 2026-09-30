<script lang="ts">
  import { get } from "svelte/store";
  import { getGraphInfo } from "../lib/api";
  import type { PageNavigationTarget } from "../lib/navigation";
  import { assetBaseDirFor, hydrateAssetMedia } from "../lib/markdown";
  import { readingNoteRelativePath, renderReadingNoteMarkdown } from "../lib/readingNotes";
  import { bookNotesContext, bookSelection, selectionForBook, jumpToBookNote, compatibleBookLocation, hasBookNoteConflicts, type BookNote } from "../lib/books";
  import {
    getBookNotesSource, newBookNoteDraft, editBookNote, useBookSelection, clearBookPassage,
    loadBookNotes, saveBookNoteDraft, deleteBookNote, reviewBookNoteRevision, chooseBookNoteCandidate, resolveBookNoteDraft,
    bookDraftChanges, updateBookDrafts, bookDraftDirty, type BookNotesSource,
  } from "../lib/bookNoteDrafts";

  let { pageId, pageTitle, active = true, focusNoteId = null, onNavigate = () => {} }: {
    pageId: string; pageTitle: string; active?: boolean; focusNoteId?: string | null;
    onNavigate?: (target: PageNavigationTarget) => void;
  } = $props();
  let source = $state.raw<BookNotesSource | null>(null);
  let error = $state("");
  let retry = $state(0);
  let pendingDelete = $state.raw<{ source: BookNotesSource; note: BookNote } | null>(null);
  let deleting = $state(false);
  let pendingResolution = $state.raw<{ source: BookNotesSource; note: BookNote } | null>(null);
  const view = $derived.by(() => {
    $bookDraftChanges;
    if (!source) return null;
    const draft = source.activeId ? source.drafts.get(source.activeId) : null;
    return { ...source, notes: [...source.notes], drafts: [...source.drafts.values()],
      draft: draft ? { ...draft } : null, dirty: draft ? bookDraftDirty(draft) : false };
  });
  const selection = $derived(view ? selectionForBook($bookSelection, view.graphPath, view.book) : null);
  const conflict = $derived(view?.notes.find(n => n.id === view.draft?.id && n.revision !== view.draft.note?.revision));
  const resolving = $derived(hasBookNoteConflicts(view?.draft?.note)
    || hasBookNoteConflicts(view?.notes.find(n => n.id === view.draft?.id)));
  $effect(() => {
    const destination = source;
    if (!destination || !active) return;
    const refresh = (event: Event) => {
      const change = (event as CustomEvent<{ graphPath: string }>).detail;
      if (change?.graphPath === destination.graphPath) void refreshSource(destination);
    };
    window.addEventListener("graph-sources-changed", refresh);
    return () => window.removeEventListener("graph-sources-changed", refresh);
  });
  $effect(() => {
    const id = pageId;
    const requestedNote = focusNoteId;
    retry;
    pendingDelete = null; pendingResolution = null;
    if (!active || !id) return;
    let disposed = false;
    source = null; error = ""; pendingDelete = null;
    void (async () => {
      try {
        const graph = await getGraphInfo();
        const book = await bookNotesContext(graph.path, id);
        if (disposed) return;
        if (book.pageId !== id) throw new Error("Book source does not match this page.");
        const destination = getBookNotesSource(graph.path, book);
        if (requestedNote) destination.activeId = destination.drafts.has(requestedNote) ? requestedNote : null;
        source = destination;
        if (!destination.activeId && !requestedNote && book.sourceAvailable !== false) newBookNoteDraft(destination);
        await loadBookNotes(destination);
        if (disposed) return;
        if (requestedNote) {
          const note = destination.notes.find(n => n.id === requestedNote);
          if (note) editBookNote(destination, note);
          else error = "This note was removed or is unavailable. Other notes and existing drafts are kept.";
        }
      } catch (e) { if (!disposed) error = `Could not open book notes. Drafts are kept. ${String(e)}`; }
    })();
    return () => { disposed = true; };
  });
  const draft = () => source?.activeId ? source.drafts.get(source.activeId) : null;
  async function refreshSource(destination: BookNotesSource) {
    try {
      destination.book = await bookNotesContext(destination.graphPath, destination.book.id);
      await loadBookNotes(destination);
    } catch (cause) {
      destination.error = `Could not refresh this book's context. Drafts are kept. ${String(cause)}`;
    }
    updateBookDrafts();
  }
  function body(value: string) { const d = draft(); if (d && !d.saving) { d.body = value; d.notice = ""; updateBookDrafts(); } }
  function useSelection() { const d = draft(); if (source && d) useBookSelection(source, d, get(bookSelection)); }
  function clearPassage() { const d = draft(); if (source && d) clearBookPassage(source, d); }
  function save() { const d = draft(); if (source && d) void saveBookNoteDraft(source, d); }
  function resolve() { const d = draft(); if (source && d) void resolveBookNoteDraft(source, d); }
  async function resolveDeletion() {
    const request = pendingResolution;
    if (!request) return;
    const d = request.source.drafts.get(request.note.id);
    if (!d || d.saving) return;
    if (d.note?.revision !== request.note.revision) {
      pendingResolution = null; return;
    }
    await resolveBookNoteDraft(request.source, d, true);
    if (pendingResolution === request) pendingResolution = null;
  }
  function newNote() { if (source) { const d = newBookNoteDraft(source); if (selection) useBookSelection(source, d, selection); } }
  function openNote(note: BookNote) {
    if (source && note.filePath.endsWith(".jsonld")) editBookNote(source, note);
    onNavigate({ id: note.notePageId });
  }
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
    return !hasBookNoteConflicts(note) && !!source && source.book.sourceAvailable !== false
      && note.status === "attached" && note.sourceSha256 === source.book.sourceSha256
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
  function preview(note: Pick<BookNote, "body" | "filePath">, graphPath: string) {
    return renderReadingNoteMarkdown(note.body, assetBaseDirFor(readingNoteRelativePath(graphPath, note.filePath)));
  }
  function media(node: HTMLElement, _body: string) {
    let cleanup = hydrateAssetMedia(node);
    return { update(_value: string) { cleanup(); cleanup = hydrateAssetMedia(node); }, destroy() { cleanup(); } };
  }
</script>

<section class="book-notes-panel" aria-label="Original book notes" data-help-context="books">
  <header>
    <h2>{view?.book.title || pageTitle}</h2>
    <p>New notes share one adjacent JSON-LD file (1.epub → 1.jsonld). Legacy Markdown notes remain supported. The original book is never rewritten.</p>
    <p>Drafts survive navigation and graph switching in this session, not closing the app. <strong>Save note before closing.</strong></p>
    {#if view?.book.sourceAvailable === false}
      <p role="status">Original unavailable. Existing notes and conflict candidates remain accessible. Keep their saved attachments; new notes, new attachments, and passage navigation require restoring the source.</p>
    {/if}
  </header>
  {#if error}<div class="error" role="alert">{error} <button onclick={() => retry++}>Retry</button></div>{/if}
  {#if !view}
    {#if !error}<p role="status">Loading book notes…</p>{/if}
  {:else}
    <div class="toolbar">
      <button disabled={view.book.sourceAvailable === false} onclick={newNote}>New note</button>
      <button disabled={view.loading} onclick={() => source && refreshSource(source)}>Refresh</button>
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
        {#if resolving}
          <p class="error">Conflicting versions need your review. Compare every candidate below, choose an attachment, and edit the merged Markdown. No version is saved automatically.</p>
          {#if !view.draft.attachmentChosen}
            <p>{view.book.sourceAvailable === false ? "Choose a candidate attachment before resolving, including deletion." : "Choose a candidate attachment, Use selection, or Whole-book note before resolving."}</p>
          {/if}
        {/if}
        {#if view.draft.attachmentChosen && view.draft.sourceSha256 !== view.book.sourceSha256}
          <p class="error">This attachment belongs to an older source. Choose a current selection or Whole-book note to reattach; keeping it preserves the old attachment.</p>
        {/if}
        {#if view.draft.quote}<blockquote>{view.draft.quote}</blockquote>{:else}<p>Whole-book note · no passage attached</p>{/if}
        <div class="toolbar">
          <button disabled={!selection || view.draft.saving} onclick={useSelection}>Use selection</button>
          <button disabled={view.draft.saving || view.book.sourceAvailable === false} onclick={clearPassage}>Whole-book note</button>
        </div>
        {#if selection}<p>{selection.quote.length} selected characters available from this book.</p>{/if}
        <textarea aria-label="Book note" rows="7" value={view.draft.body} disabled={view.draft.saving}
          oninput={e => body(e.currentTarget.value)} placeholder="Write a Markdown note…"></textarea>
        <div class="toolbar">
          {#if resolving}
            <button class="save" disabled={view.draft.saving || !view.draft.body.trim() || !view.draft.attachmentChosen || !!conflict} onclick={resolve}>
              {view.draft.saving ? "Resolving…" : "Resolve with merged note"}
            </button>
          {:else}
            <button class="save" disabled={view.draft.saving || !view.draft.body.trim() || (!!view.draft.note && !view.dirty)} onclick={save}>
              {view.draft.saving ? "Saving…" : "Save note"}
            </button>
          {/if}
          <span>{view.dirty ? "Unsaved draft" : view.draft.note ? "Saved" : "New draft"}</span>
        </div>
        {#if view.draft.error}<div class="error" role="alert">{view.draft.error}</div>{/if}
        {#if view.draft.notice}<p role="status">{view.draft.notice}</p>{/if}
        {#if conflict && !hasBookNoteConflicts(conflict)}
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
          {#if hasBookNoteConflicts(note)}
            <h4>Conflict · {note.conflicts.length} candidates</h4>
            <p>Review all versions, including removed versions. Selecting a candidate only prepares a draft; it does not resolve the conflict.</p>
            <div class="conflict-candidates">
              {#each note.conflicts as candidate, index (candidate.revision)}
                <section class="conflict-candidate" aria-label={`Candidate ${index + 1}`}>
                  <h4>Candidate {index + 1} · {candidate.deleted ? "Deleted" : "Edited"}</h4>
                  <small>Updated {candidate.updatedAt} · Revision {candidate.revision}</small>
                  {#if candidate.deleted}<p>This version removes the note.</p>{/if}
                  {#if candidate.quote}<blockquote>{candidate.quote}</blockquote>{:else}<p>No quoted passage</p>{/if}
                  <p class="anchor">Attachment: {candidate.locator ? JSON.stringify(candidate.locator) : "Whole book"}</p>
                  <small>Source SHA-256: {candidate.sourceSha256}</small>
                  <div class="markdown-preview" use:media={candidate.body}>{@html preview({ body: candidate.body, filePath: note.filePath }, view.graphPath)}</div>
                  {#if !candidate.deleted}
                    <div class="toolbar">
                      <button disabled={view.drafts.some(d => d.id === note.id && d.saving)}
                        onclick={() => source && chooseBookNoteCandidate(source, note, candidate)}>Use candidate {index + 1} text and attachment</button>
                    </div>
                  {/if}
                  <button disabled={view.drafts.some(d => d.id === note.id && d.saving)}
                    onclick={() => source && chooseBookNoteCandidate(source, note, candidate, false)}>Use candidate {index + 1} attachment only</button>
                </section>
              {/each}
            </div>
            <div class="toolbar">
              <button onclick={() => source && editBookNote(source, note)}>Edit merge draft</button>
              <button onclick={() => openNote(note)}>Open note</button>
              <button disabled={view.drafts.some(d => d.id === note.id && d.saving)} onclick={() => {
                if (source) {
                  const d = editBookNote(source, note);
                  reviewBookNoteRevision(source, d, note.revision);
                  pendingResolution = { source, note };
                }
              }}>Resolve as deleted…</button>
            </div>
          {:else}
          <p class="status">{note.status === "orphaned" || note.sourceSha256 !== view.book.sourceSha256 ? "Orphaned · source changed" : note.locator && !canJump(note) ? "Passage unavailable · reader version changed" : note.locator ? "Passage note" : "Whole-book note"}</p>
          {#if note.quote}<blockquote>{note.quote}</blockquote>{/if}
          <div class="markdown-preview" use:media={note.body}>{@html preview(note, view.graphPath)}</div>
          <div class="toolbar">
            <button onclick={() => source && editBookNote(source, note)}>Edit note</button>
            <button disabled={!canJump(note)} onclick={() => jump(note)}>Return to passage</button>
            <button onclick={() => openNote(note)}>Open note</button>
            <button disabled={view.drafts.some(d => d.id === note.id && d.saving)} onclick={() => {
              if (source) pendingDelete = { source, note };
            }}>Remove</button>
          </div>
          {/if}
          <small>{note.filePath}</small>
        </article>
      {/each}
    </div>
  {/if}
  {#if pendingDelete}
    <div class="remove-confirm" role="alertdialog" aria-label="Remove book note" aria-modal="false" tabindex="-1">
      <p>Remove this saved note? The original book will not change. Unsaved edits are kept as a new draft.</p>
      <button disabled={deleting} onclick={remove}>{deleting ? "Removing…" : "Remove saved note"}</button>
      <button disabled={deleting} onclick={() => pendingDelete = null}>Cancel</button>
    </div>
  {/if}
  {#if pendingResolution}
    <div class="remove-confirm" role="alertdialog" aria-label="Resolve book note as deleted" aria-modal="false" tabindex="-1">
      <p>Resolve all reviewed candidates by deleting this note? This includes the edited versions.
        {pendingResolution.source.book.sourceAvailable === false
          ? "The original is unavailable: explicitly choose a candidate attachment to retain in the deletion record."
          : "Deletion is recorded against the current book with no passage attachment."}
        Your composer text is kept as an unsaved draft; the original book will not change.</p>
      <button disabled={pendingResolution.source.drafts.get(pendingResolution.note.id)?.saving
        || (pendingResolution.source.book.sourceAvailable === false
          && !view?.drafts.find(d => d.id === pendingResolution?.note.id)?.attachmentChosen)}
        onclick={resolveDeletion}>Confirm deletion resolution</button>
      <button disabled={pendingResolution.source.drafts.get(pendingResolution.note.id)?.saving} onclick={() => pendingResolution = null}>Cancel</button>
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
  .conflict-candidates { display:grid; grid-template-columns:repeat(auto-fit,minmax(min(240px,100%),1fr)); gap:10px; }
  .conflict-candidate { min-width:0; padding:10px; border:1px solid var(--border); border-radius:5px; background:var(--bg-secondary); }
  h4 { margin:0 0 6px; font-size:13px; }
  .anchor { overflow-wrap:anywhere; font-size:12px; }
  .markdown-preview { overflow-wrap:anywhere; margin:10px 0; }
  .markdown-preview :global(img) { max-width:100%; }
  .remove-confirm,.revision { padding:10px; border:1px solid var(--border); background:var(--bg-secondary); border-radius:5px; }
  .remove-confirm { position:sticky; bottom:0; }
</style>
