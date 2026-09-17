<script lang="ts">
  import { tick, untrack } from "svelte";
  import { get } from "svelte/store";
  import { getGraphInfo, getPage } from "../lib/api";
  import { hydrateAssetMedia } from "../lib/markdown";
  import type { PageNavigationTarget } from "../lib/navigation";
  import { readingSelection } from "../lib/readingSelection";
  import {
    readingNoteSelection, readingNotePassageTarget, readingNoteLocationTarget, renderReadingNoteBody, type ReadingNote,
  } from "../lib/readingNotes";
  import {
    getReadingNotesSource, newReadingNoteDraft, editReadingNote, applyReadingNoteFocusRequest, readingNoteDirty, loadReadingNotes,
    saveReadingNoteDraft, reattachReadingNoteDraft, readingNoteChanges, updateReadingNotes,
    useReviewedReadingNoteRevision,
    type ReadingNotesSource,
  } from "../lib/readingNoteDrafts";

  let {
    pageId, pageTitle, active = true, onNavigate = () => {},
    initialNoteLabel = null, initialNotePageId = null, noteFocusTrigger = 0,
  }: {
    pageId: string; pageTitle: string; active?: boolean;
    onNavigate?: (target: PageNavigationTarget) => void;
    initialNoteLabel?: string | null; initialNotePageId?: string | null; noteFocusTrigger?: number;
  } = $props();

  let source = $state.raw<ReadingNotesSource | null>(null);
  let sourceError = $state("");
  let refreshTrigger = $state(0);
  let textarea = $state<HTMLTextAreaElement | undefined>();
  const view = $derived.by(() => {
    $readingNoteChanges;
    if (!source) return null;
    const draft = source.activeDraftId ? source.drafts.get(source.activeDraftId) : null;
    return {
      ...source, notes: [...source.notes], drafts: [...source.drafts.values()],
      draft: draft ? { ...draft } : null, dirty: draft ? readingNoteDirty(draft) : false,
    };
  });
  const currentSelection = $derived(
    !$readingSelection.error && $readingSelection.selection?.pageId === pageId ? $readingSelection.selection : null,
  );
  const selectionError = $derived($readingSelection.pageIds.includes(pageId) ? $readingSelection.error : null);
  const canReattach = $derived(view?.draft?.note?.status === "ambiguous" || view?.draft?.note?.status === "orphaned");
  const changedSavedNote = $derived(view?.notes.find((note) => note.id === view.draft?.id
    && note.revision !== view.draft?.note?.revision));

  $effect(() => {
    const id = pageId;
    const title = pageTitle;
    const requestedLabel = initialNotePageId === id ? initialNoteLabel : null;
    const requestedTrigger = noteFocusTrigger;
    refreshTrigger;
    if (!active) return;
    const captured = untrack(() => get(readingSelection));
    let disposed = false;
    source = null;
    sourceError = "";
    void (async () => {
      try {
        const graph = await getGraphInfo();
        if (disposed) return;
        const destination = getReadingNotesSource(graph.path, id, title);
        source = destination;
        if (!destination.activeDraftId && id && !requestedLabel) {
          const draft = newReadingNoteDraft(destination);
          try {
            draft.selection = readingNoteSelection(captured, id);
          } catch (error) {
            draft.error = String(error);
          }
          updateReadingNotes();
        }
        const loaded = loadReadingNotes(destination);
        if (requestedLabel) {
          await loaded;
          if (disposed) return;
          if (!destination.listError) {
            applyReadingNoteFocusRequest(destination, id, requestedLabel, requestedTrigger);
          }
        }
        if (id) {
          const page = await getPage({ id });
          if (disposed) return;
          destination.pageTitle = page.title;
          updateReadingNotes();
        }
      } catch (error) {
        if (!disposed) sourceError = `Could not read the current source. Your drafts are kept. ${String(error)}`;
      }
    })();
    return () => { disposed = true; };
  });

  function activeDraft() {
    return source?.activeDraftId ? source.drafts.get(source.activeDraftId) : null;
  }

  function newNote() {
    if (!source || !pageId) return;
    const draft = newReadingNoteDraft(source);
    try {
      draft.selection = readingNoteSelection(get(readingSelection), pageId);
    } catch (error) {
      draft.error = String(error);
    }
    updateReadingNotes();
    void focusComposer(source);
  }

  async function focusComposer(destination: ReadingNotesSource) {
    await tick();
    if (source !== destination) return;
    textarea?.focus();
    textarea?.scrollIntoView({ block: "nearest" });
  }

  function editNote(note: ReadingNote) {
    if (!source) return;
    editReadingNote(source, note);
    void focusComposer(source);
  }

  function useSelection() {
    const draft = activeDraft();
    if (!draft || draft.saving) return;
    try {
      const selection = readingNoteSelection(get(readingSelection), pageId);
      if (!selection) throw new Error("Select words or a passage in the current page first.");
      if (!draft.note && draft.sourcePageId !== pageId) throw new Error("Return to this draft's source page first.");
      draft.selection = selection;
      draft.error = null;
      draft.notice = "";
    } catch (error) {
      draft.error = String(error);
    }
    updateReadingNotes();
  }

  function clearSelection() {
    const draft = activeDraft();
    if (!draft || draft.saving) return;
    draft.selection = null;
    draft.notice = "";
    updateReadingNotes();
  }

  function setBody(body: string) {
    const draft = activeDraft();
    if (!draft) return;
    draft.body = body;
    draft.notice = "";
    updateReadingNotes();
  }

  function selectDraft(id: string) {
    if (!source) return;
    source.activeDraftId = id;
    updateReadingNotes();
  }

  function changeScope(scope: string) {
    if (!source) return;
    source.scope = scope === "all" ? "all" : "page";
    void loadReadingNotes(source);
  }

  function save() {
    const destination = source;
    const draft = activeDraft();
    if (destination && draft) void saveReadingNoteDraft(destination, draft);
  }

  function reattach() {
    const destination = source;
    const draft = activeDraft();
    if (destination && draft) void reattachReadingNoteDraft(destination, draft, pageId);
  }

  function useReviewedVersion(revision: string) {
    const draft = activeDraft();
    if (source && draft) useReviewedReadingNoteRevision(source, draft, revision);
  }

  function openPassage(note: ReadingNote) {
    const target = readingNotePassageTarget(note);
    if (target) window.dispatchEvent(new CustomEvent("navigate-page", { detail: target }));
    else if (note.source.pageId) onNavigate({ id: note.source.pageId });
  }

  function openNote(note: ReadingNote) {
    const target = readingNoteLocationTarget(note);
    if (target) window.dispatchEvent(new CustomEvent("navigate-page", { detail: target }));
    else onNavigate({ id: note.notePageId });
  }

  function previewMedia(node: HTMLElement, _body: string) {
    let cleanup = hydrateAssetMedia(node);
    return {
      update(_nextBody: string) { cleanup(); cleanup = hydrateAssetMedia(node); },
      destroy() { cleanup(); },
    };
  }
</script>

<section class="reading-notes-panel" aria-label="Reading notes">
  <header class="notes-header">
    <h2>{view?.pageTitle || pageTitle || "Reading notes"}</h2>
    <p class="storage-notice">New notes are footnotes in the source Markdown file. Save notes before closing the app.</p>
    <p class="note-help">Only note markers and footnotes are added; book words stay unchanged. Existing separate note files remain supported.</p>
  </header>

  {#if sourceError}
    <div class="notes-error" role="alert">{sourceError}</div>
    <button type="button" onclick={() => refreshTrigger++}>Retry source</button>
  {/if}

  {#if !view}
    {#if !sourceError}<p class="shimmer" role="status">Loading notes…</p>{/if}
  {:else}
    {#if view.focusError}<div class="notes-error" role="alert">{view.focusError}</div>{/if}
    <div class="notes-toolbar">
      <label>
        <span class="control-label">Notes scope</span>
        <select aria-label="Notes scope" value={view.scope} onchange={(event) => changeScope(event.currentTarget.value)}>
          <option value="page" disabled={!pageId}>Current source</option>
          <option value="all">All notes</option>
        </select>
      </label>
      <button type="button" onclick={newNote} disabled={!pageId || (!!view.draft && !view.draft.note && !view.dirty)}>
        New note
      </button>
      <button type="button" onclick={() => source && loadReadingNotes(source)} disabled={view.loading}>Refresh</button>
    </div>

    {#if view.drafts.length > 1}
      <label class="draft-picker">
        <span class="control-label">Drafts kept for this source</span>
        <select aria-label="Note draft" value={view.activeDraftId ?? ""} onchange={(event) => selectDraft(event.currentTarget.value)}>
          {#each view.drafts as draft}
            <option value={draft.id}>
              {readingNoteDirty(draft) ? "Unsaved · " : draft.note ? "Saved · " : "New · "}{draft.body.trim().slice(0, 55) || "Untitled note"}
            </option>
          {/each}
        </select>
      </label>
    {/if}

    {#if view.draft}
      <div class="note-composer">
        <div class="composer-heading">
          <h3>{view.draft.note ? "Edit note" : "New reading note"}</h3>
          {#if view.draft.note}<span class="note-status">{view.draft.note.status}</span>{/if}
        </div>
        {#if view.draft.note}
          <p class="note-source">Source: {view.draft.note.source.pageTitle || "Unavailable source"}</p>
        {/if}

        {#if view.draft.selection}
          <p class="control-label">{view.draft.note ? `Proposed passage · ${view.pageTitle}` : "Selected quote"}</p>
          <blockquote class="quote-preview">{view.draft.selection.text}</blockquote>
        {:else if view.draft.note?.quote}
          <p class="control-label">Source quote</p>
          <blockquote class="quote-preview">{view.draft.note.quote}</blockquote>
        {:else if currentSelection && !view.draft.note}
          <p class="control-label">Current selection · choose Use selection to attach</p>
          <blockquote class="quote-preview">{currentSelection.text}</blockquote>
        {:else}
          <p class="note-help">Page-level note. Select words or a passage to attach a quote.</p>
        {/if}

        {#if !view.draft.note || canReattach}
          <div class="selection-actions">
            <button type="button" onclick={useSelection} disabled={!currentSelection || view.draft.saving}>Use selection</button>
            {#if view.draft.selection}
              <button type="button" onclick={clearSelection} disabled={view.draft.saving}>
                {view.draft.note ? "Cancel reattachment" : "Make page-level note"}
              </button>
            {/if}
          </div>
          {#if selectionError}<p class="notes-error" role="alert">{selectionError}</p>{/if}
        {/if}

        {#if canReattach && view.draft.selection}
          <p class="note-help">Confirm this replacement quote on the current source. Reattachment does not save body edits.</p>
          <button type="button" class="reattach-button" onclick={reattach} disabled={view.draft.saving}>Reattach to selection</button>
        {/if}

        <label for="reading-note-body" class="control-label">Your note · Markdown supported</label>
        <textarea id="reading-note-body" aria-label="Reading note" bind:this={textarea}
          value={view.draft.body} oninput={(event) => setBody(event.currentTarget.value)}
          placeholder="What do you want to remember?" rows="5"></textarea>

        <div class="save-actions">
          <button type="button" class="save-note" onclick={save} disabled={view.draft.saving || !view.draft.body.trim()}>
            Save note
          </button>
          <span class="draft-status" role="status">
            {view.draft.saving ? "Saving…" : view.dirty ? "Unsaved draft" : view.draft.note ? "Saved" : "Not saved yet"}
          </span>
        </div>
        {#if view.dirty}<p class="note-help">Draft kept while navigating, not after closing the app.</p>{/if}
        {#if view.draft.notice}<p class="note-help" role="status">{view.draft.notice}</p>{/if}
        {#if view.draft.error}<div class="notes-error" role="alert">{view.draft.error}</div>{/if}
        {#if changedSavedNote}
          <details class="revision-review">
            <summary>Review changed saved version</summary>
            <p class="note-help">The saved note or attachment changed. Compare the saved version below. Your draft stays unchanged until you explicitly save it.</p>
            <p class="note-source">Source: {changedSavedNote.source.pageTitle} · {changedSavedNote.status}</p>
            {#if changedSavedNote.statusMessage}<p class="note-help">{changedSavedNote.statusMessage}</p>{/if}
            {#if changedSavedNote.quote}<blockquote class="quote-preview">{changedSavedNote.quote}</blockquote>{/if}
            <div class="note-body rendered-content">{@html renderReadingNoteBody(changedSavedNote, view.graphPath)}</div>
            <p class="note-file">File: <code>{changedSavedNote.filePath}</code></p>
            <div class="card-actions">
              <button type="button" onclick={() => openNote(changedSavedNote)}>Open saved note</button>
              <button type="button" disabled={view.draft.saving} onclick={() => useReviewedVersion(changedSavedNote.revision)}>
                Use reviewed version for retry
              </button>
            </div>
          </details>
        {/if}
        {#if view.draft.note}
          <p class="note-file">{view.draft.note.storage === "inline" ? "Source file" : "Legacy note file"}: <code>{view.draft.note.filePath}</code></p>
        {/if}
      </div>
    {:else}
      <p class="note-help">Open a page, book, or journal day to write a note, or edit a saved note below.</p>
    {/if}

    <div class="saved-notes">
      <h3>{view.scope === "all" ? "All saved notes" : "Notes on this source"}</h3>
      {#if view.loading}<p class="shimmer" role="status">Loading saved notes…</p>{/if}
      {#if view.listError}<div class="notes-error" role="alert">{view.listError}</div>{/if}
      {#if view.warnings.length}
        <div class="notes-warning" role="alert">
          <strong>Some note files need attention</strong>
          <ul>{#each view.warnings as warning}<li>{warning}</li>{/each}</ul>
        </div>
      {/if}
      {#if !view.notes.length && !view.loading && !view.listError}
        <p class="note-help">
          {view.warnings.length ? "No readable saved notes in this scope." : "No saved notes yet."}
          {view.scope === "page" ? "All notes also includes notes whose sources are missing." : ""}
        </p>
      {/if}
      {#each view.notes as note (note.id)}
        <article class="reading-note-card" aria-label={`Note on ${note.source.pageTitle || "unavailable source"}`}>
          <div class="card-heading">
            <h4>{note.source.pageTitle || "Unavailable source"}</h4>
            <span class="note-status" class:needs-attention={note.status === "orphaned" || note.status === "ambiguous"}>
              {note.status}
            </span>
          </div>
          {#if note.statusMessage}<p class="note-help">{note.statusMessage}</p>{/if}
          {#if note.quote}<blockquote class="source-quote">{note.quote}</blockquote>
          {:else}<p class="note-help">Page-level note</p>{/if}
          <div class="note-body rendered-content" use:previewMedia={note.body}>{@html renderReadingNoteBody(note, view.graphPath)}</div>
          <p class="note-file">{note.storage === "inline" ? `Footnote ${note.footnoteLabel?.replace("grafium-note-", "") ?? ""} · source file` : "Legacy note file"}: <code>{note.filePath}</code></p>
          <div class="card-actions">
            <button type="button" onclick={() => editNote(note)}>Edit note</button>
            <button type="button" onclick={() => openNote(note)}>Open note</button>
            <button type="button" disabled={!note.source.pageId} onclick={() => note.source.pageId && onNavigate({ id: note.source.pageId })}>
              Open source
            </button>
            {#if readingNotePassageTarget(note)}
              <button type="button" onclick={() => openPassage(note)}>Go to passage</button>
            {/if}
          </div>
        </article>
      {/each}
    </div>
  {/if}
</section>

<style>
  .reading-notes-panel { flex: 1; min-width: 0; min-height: 0; overflow-y: auto; overflow-x: hidden; display: flex; flex-direction: column; gap: 14px; padding: 0 2px 16px; color: var(--text-primary); font-size: 13px; line-height: 1.5; }
  .reading-notes-panel > * { flex-shrink: 0; }
  h2, h3, h4, p { margin: 0; }
  h2 { font-size: 15px; overflow-wrap: anywhere; }
  h3, h4 { font-size: 13px; font-weight: 600; }
  .storage-notice, .note-help, .note-file, .note-source { color: var(--text-secondary); font-size: 12px; }
  .storage-notice { margin-top: 4px; }
  .notes-toolbar, .selection-actions, .save-actions, .card-actions, .composer-heading, .card-heading { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; }
  .notes-toolbar { align-items: flex-end; }
  .notes-toolbar label { flex: 1; min-width: 125px; }
  .control-label { display: block; color: var(--text-secondary); font-size: 12px; margin-bottom: 4px; }
  select, textarea, button { font: inherit; border: 1px solid var(--border); border-radius: 5px; color: var(--text-primary); }
  select, textarea { background: var(--bg-primary); width: 100%; min-width: 0; box-sizing: border-box; }
  select { padding: 6px; }
  button { background: var(--bg-tertiary); padding: 6px 9px; cursor: pointer; min-height: 32px; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  button:focus-visible, select:focus-visible, textarea:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .note-composer { display: flex; flex-direction: column; gap: 9px; }
  .composer-heading, .card-heading { justify-content: space-between; }
  .note-status { font-size: 11px; border: 1px solid var(--border); border-radius: 4px; padding: 1px 5px; color: var(--text-secondary); }
  .note-status.needs-attention { color: var(--warning-color, var(--text-primary)); }
  blockquote { border-left: 1px solid var(--border); padding: 5px 10px; margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; color: var(--text-secondary); }
  .quote-preview { max-height: 120px; overflow-y: auto; }
  .source-quote { margin: 10px 0; max-height: 180px; overflow-y: auto; }
  textarea { padding: 9px; min-height: 105px; resize: vertical; line-height: 1.6; }
  .save-note { background: var(--btn-primary-bg, var(--accent)); color: var(--btn-primary-fg, var(--bg-primary)); border-color: transparent; }
  .save-note:hover:not(:disabled) { background: var(--btn-primary-bg, var(--accent)); filter: brightness(1.08); }
  .draft-status { color: var(--text-secondary); font-size: 12px; }
  .notes-error, .notes-warning { padding: 9px; border: 1px solid var(--border); border-radius: 5px; overflow-wrap: anywhere; }
  .notes-error { color: var(--danger, var(--text-primary)); }
  .notes-warning ul { padding-left: 18px; margin: 6px 0 0; }
  .saved-notes { border-top: 1px solid var(--border); padding-top: 14px; }
  .saved-notes > h3 { margin-bottom: 8px; }
  .reading-note-card { padding: 14px 0; border-bottom: 1px solid var(--border); }
  .reading-note-card h4 { overflow-wrap: anywhere; min-width: 0; flex: 1; }
  .note-body { overflow-wrap: anywhere; min-width: 0; }
  .note-body :global(pre), .note-body :global(table) { max-width: 100%; overflow-x: auto; }
  .note-body :global(img), .note-body :global(video) { max-width: 100%; height: auto; }
  .note-body :global(audio) { max-width: 100%; }
  .note-file { overflow-wrap: anywhere; margin-top: 8px; }
  .note-file code { font-size: 11px; }
  .card-actions { margin-top: 10px; }
  .revision-review { border: 1px solid var(--border); border-radius: 5px; padding: 9px; }
  .revision-review summary { cursor: pointer; font-weight: 600; }
  .revision-review[open] > :not(summary) { margin-top: 9px; }
</style>
