import { writable } from "svelte/store";
import { getGraphInfo } from "./api";
import { flushPageEditors, reloadPageEditors, withPageEditorsLocked, markPageEditorsStale } from "./editorPersistence";
import type { ReadingSelection } from "./readingSelection";
import {
  cloneReadingSelection, readingNoteCreate, readingNotesList, readingNoteUpdate, readingNoteReattach,
  readingNoteDelete, readingNotesDeleteForPage,
  type ReadingNote, type ReadingNotesDeleteResult,
} from "./readingNotes";

export interface ReadingNoteDraft {
  id: string;
  sourcePageId: string;
  selection: ReadingSelection | null;
  selectionPinned: boolean;
  selectionError: string | null;
  body: string;
  savedBody: string;
  note: ReadingNote | null;
  saving: boolean;
  error: string | null;
  notice: string;
}

export interface ReadingNotesSource {
  graphPath: string;
  pageId: string;
  pageTitle: string;
  scope: "page" | "all";
  drafts: Map<string, ReadingNoteDraft>;
  activeDraftId: string | null;
  notes: ReadingNote[];
  warnings: string[];
  listError: string | null;
  loading: boolean;
  deleting: boolean;
  loadGeneration: number;
  lastFocusRequest: string | null;
  focusError: string | null;
}

// Unsaved work belongs to a graph/source, not a mounted tab. This is session memory, not a backup.
const sources = new Map<string, ReadingNotesSource>();
const savedDrafts = new Map<string, ReadingNoteDraft>();
const recentWrites = new Map<string, Map<string, { generation: number; note: ReadingNote | null }>>();
let writeGeneration = 0;
export const readingNoteChanges = writable(0);
export function updateReadingNotes(): void { readingNoteChanges.update((version) => version + 1); }

export function getReadingNotesSource(graphPath: string, pageId: string, pageTitle: string): ReadingNotesSource {
  const key = JSON.stringify([graphPath, pageId]);
  let source = sources.get(key);
  if (!source) {
    source = {
      graphPath, pageId, pageTitle, scope: pageId ? "page" : "all", drafts: new Map(), activeDraftId: null,
      notes: [], warnings: [], listError: null, loading: false,
      deleting: [...sources.values()].some((item) => item.graphPath === graphPath && item.deleting), loadGeneration: 0,
      lastFocusRequest: null, focusError: null,
    };
    sources.set(key, source);
  }
  if (pageTitle) source.pageTitle = pageTitle;
  return source;
}

export function readingNoteDirty(draft: ReadingNoteDraft): boolean {
  return draft.body !== draft.savedBody || !!draft.selection;
}

export function newReadingNoteDraft(source: ReadingNotesSource, selection: ReadingSelection | null = null): ReadingNoteDraft {
  if (selection && selection.pageId !== source.pageId) throw new Error("This selection belongs to another page.");
  source.focusError = null;
  const draft: ReadingNoteDraft = {
    id: crypto.randomUUID(), sourcePageId: source.pageId, selection: cloneReadingSelection(selection),
    selectionPinned: false, selectionError: null,
    body: "", savedBody: "", note: null, saving: false, error: null, notice: "",
  };
  source.drafts.set(draft.id, draft);
  source.activeDraftId = draft.id;
  updateReadingNotes();
  return draft;
}

export function editReadingNote(source: ReadingNotesSource, note: ReadingNote): ReadingNoteDraft {
  source.focusError = null;
  const key = JSON.stringify([source.graphPath, note.id]);
  let draft = savedDrafts.get(key) ?? source.drafts.get(note.id);
  if (!draft) {
    draft = {
      id: note.id, sourcePageId: note.source.pageId ?? "", selection: null, selectionPinned: true, selectionError: null,
      body: note.body, savedBody: note.body, note, saving: false, error: null, notice: "",
    };
    savedDrafts.set(key, draft);
  } else if (!readingNoteDirty(draft) && !draft.saving) {
    draft.note = note;
    draft.body = note.body;
    draft.savedBody = note.body;
    draft.sourcePageId = note.source.pageId ?? "";
  }
  source.drafts.set(draft.id, draft);
  savedDrafts.set(key, draft);
  source.activeDraftId = draft.id;
  updateReadingNotes();
  return draft;
}

export function focusReadingNoteLabel(source: ReadingNotesSource, pageId: string, label: string): ReadingNoteDraft {
  const matching = source.notes.filter((note) => note.source.pageId === pageId && note.footnoteLabel === label);
  if (matching.length !== 1) {
    throw new Error(matching.length
      ? `Footnote ${label} occurs more than once. Review the source file; no note was selected.`
      : `Footnote ${label} was not found in this source. No note was created.`);
  }
  return editReadingNote(source, matching[0]);
}

export function useReviewedReadingNoteRevision(
  source: ReadingNotesSource, draft: ReadingNoteDraft, reviewedRevision: string,
): void {
  if (draft.saving) return;
  const note = source.notes.find((note) => note.id === draft.id && note.revision === reviewedRevision);
  if (!note) {
    draft.error = "The saved version changed again. Refresh and review it before retrying.";
  } else {
    draft.note = note;
    draft.savedBody = note.body;
    draft.sourcePageId = note.source.pageId ?? "";
    draft.error = null;
    draft.notice = "Saved version reviewed. Your draft is unchanged; choose Save note to retry.";
    savedDrafts.set(JSON.stringify([source.graphPath, draft.id]), draft);
  }
  updateReadingNotes();
}

export function applyReadingNoteFocusRequest(source: ReadingNotesSource, pageId: string, label: string, trigger: number): void {
  if (source.pageId !== pageId) return;
  const key = JSON.stringify([pageId, label, trigger]);
  if (source.lastFocusRequest === key) return;
  source.lastFocusRequest = key;
  source.focusError = null;
  try {
    focusReadingNoteLabel(source, pageId, label);
  } catch (error) {
    source.focusError = errorText(error);
  }
  updateReadingNotes();
}

export async function loadReadingNotes(source: ReadingNotesSource): Promise<void> {
  const generation = ++source.loadGeneration;
  const writesAtStart = writeGeneration;
  const pageId = source.scope === "page" ? source.pageId : undefined;
  source.loading = true;
  source.listError = null;
  updateReadingNotes();
  try {
    const result = await readingNotesList(source.graphPath, pageId);
    if (generation !== source.loadGeneration) return;
    const writes = [...recentWrites.get(source.graphPath)?.entries() ?? []]
      .filter(([, write]) => write.generation > writesAtStart);
    const writtenIds = new Set(writes.map(([id]) => id));
    source.notes = [
      ...writes.map(([, write]) => write.note)
        .filter((note): note is ReadingNote => !!note && (!pageId || note.source.pageId === pageId)),
      ...result.notes.filter((note) => !writtenIds.has(note.id)),
    ];
    source.warnings = result.warnings;
  } catch (error) {
    if (generation === source.loadGeneration) {
      source.listError = `Could not load notes. Your drafts are unchanged. Retry when the graph is available. ${errorText(error)}`;
    }
  } finally {
    if (generation === source.loadGeneration) {
      source.loading = false;
      updateReadingNotes();
    }
  }
}

function errorText(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) return String(error.message);
  return String(error);
}

async function assertGraph(graphPath: string): Promise<void> {
  if ((await getGraphInfo()).path !== graphPath) {
    throw new Error("The graph changed. Return to the original graph and try again.");
  }
}

async function withNoteSourceEditors<T>(graphPath: string, pageIds: string[], operation: () => Promise<T>): Promise<T> {
  await assertGraph(graphPath);
  const ids = [...new Set(pageIds.filter(Boolean))].sort();
  const lock = (index: number): Promise<T> => index < ids.length
    ? withPageEditorsLocked(ids[index], () => lock(index + 1))
    : (async () => {
      for (const id of ids) {
        await assertGraph(graphPath);
        await flushPageEditors(id);
      }
      await assertGraph(graphPath);
      return operation();
    })();
  return lock(0);
}

async function refreshWrittenSources(graphPath: string, pageIds: string[], draft: ReadingNoteDraft): Promise<void> {
  try {
    // A switched graph has no old source editor to refresh; the receipt still belongs to the old draft.
    if ((await getGraphInfo()).path !== graphPath) return;
    await Promise.all([...new Set(pageIds.filter(Boolean))].map(reloadPageEditors));
  } catch (error) {
    for (const pageId of new Set(pageIds.filter(Boolean))) markPageEditorsStale(pageId);
    draft.error = `Note saved, but the source editor could not refresh. It is protected from stale edits. Retry Save note to reload it. ${errorText(error)}`;
  }
}

function publishSavedNote(graphPath: string, draft: ReadingNoteDraft, note: ReadingNote): void {
  draft.note = note;
  draft.savedBody = note.body;
  savedDrafts.set(JSON.stringify([graphPath, note.id]), draft);
  let writes = recentWrites.get(graphPath);
  if (!writes) recentWrites.set(graphPath, writes = new Map());
  writes.set(note.id, { generation: ++writeGeneration, note });
  for (const source of sources.values()) {
    if (source.graphPath !== graphPath) continue;
    source.notes = source.notes.filter((existing) => existing.id !== note.id);
    if (source.scope === "all" || source.pageId === note.source.pageId) source.notes.unshift(note);
  }
}

/** Snapshot every argument before awaiting, and settle back into the original draft even after navigation. */
export async function saveReadingNoteDraft(source: ReadingNotesSource, draft: ReadingNoteDraft): Promise<void> {
  if (draft.saving) return;
  if (source.deleting) {
    draft.error = "Wait for note deletion to finish before saving.";
    updateReadingNotes();
    return;
  }
  const { graphPath } = source;
  const { id, body, sourcePageId } = draft;
  const revision = draft.note?.revision;
  const affectedPages = !draft.note || draft.note.storage === "inline"
    ? [sourcePageId] : [draft.note.notePageId];
  const selection = cloneReadingSelection(draft.selection);
  draft.error = null;
  draft.notice = "";
  if (!draft.note && draft.selectionError) {
    draft.error = draft.selectionError;
    updateReadingNotes();
    return;
  }
  if (!body.trim()) {
    draft.error = "Write a note before saving.";
    updateReadingNotes();
    return;
  }
  draft.saving = true;
  updateReadingNotes();
  try {
    await withNoteSourceEditors(graphPath, affectedPages, async () => {
      if (!revision) {
      if (!sourcePageId) throw new Error("Open a page or select a journal day before creating a note.");
      if (selection && selection.pageId !== sourcePageId) throw new Error("The selected quote belongs to another page.");
      }
      const note = revision
        ? await readingNoteUpdate(graphPath, id, revision, body)
        : await readingNoteCreate(graphPath, id, sourcePageId, selection, body);
      publishSavedNote(graphPath, draft, note);
      if (!revision) draft.selection = null;
      draft.notice = draft.body === note.body ? "Saved in graph Markdown." : "Saved. Newer edits are still unsaved.";
      await refreshWrittenSources(graphPath, affectedPages, draft);
    });
  } catch (error) {
    draft.error = `Could not save note. Your draft is kept in this session. ${errorText(error)}`;
    if (/revision|conflict/i.test(errorText(error))) {
      draft.error += " Use Refresh to review the saved note before retrying.";
    }
  } finally {
    draft.saving = false;
    updateReadingNotes();
  }
}

export async function reattachReadingNoteDraft(
  source: ReadingNotesSource, draft: ReadingNoteDraft, currentPageId: string,
): Promise<void> {
  if (draft.saving || !draft.note) return;
  if (source.deleting) {
    draft.error = "Wait for note deletion to finish before reattaching.";
    updateReadingNotes();
    return;
  }
  const { graphPath } = source;
  const { id } = draft;
  const revision = draft.note.revision;
  const affectedPages = draft.note.storage === "inline"
    ? [draft.sourcePageId, currentPageId] : [draft.note.notePageId, currentPageId];
  const selection = cloneReadingSelection(draft.selection);
  draft.error = null;
  draft.notice = "";
  if (!selection || selection.pageId !== currentPageId || currentPageId !== source.pageId) {
    draft.error = "Use a selection from the currently viewed page before reattaching.";
    updateReadingNotes();
    return;
  }
  draft.saving = true;
  updateReadingNotes();
  try {
    await withNoteSourceEditors(graphPath, affectedPages, async () => {
      const note = await readingNoteReattach(graphPath, id, revision, currentPageId, selection);
      publishSavedNote(graphPath, draft, note);
      draft.sourcePageId = currentPageId;
      draft.selection = null;
      draft.notice = readingNoteDirty(draft) ? "Passage reattached. Body edits are still unsaved." : "Passage reattached. Note body unchanged.";
      await refreshWrittenSources(graphPath, affectedPages, draft);
    });
  } catch (error) {
    draft.error = `Could not reattach note. Your draft and selected quote are kept. ${errorText(error)}`;
  } finally {
    draft.saving = false;
    updateReadingNotes();
  }
}

function publishDeletedNotes(graphPath: string, notes: ReadingNote[]): void {
  let writes = recentWrites.get(graphPath);
  if (!writes) recentWrites.set(graphPath, writes = new Map());
  for (const note of notes) {
    writes.set(note.id, { generation: ++writeGeneration, note: null });
    savedDrafts.delete(JSON.stringify([graphPath, note.id]));
    const retained = new Map<ReadingNoteDraft, ReadingNoteDraft | null>();
    for (const source of sources.values()) {
      if (source.graphPath !== graphPath) continue;
      source.notes = source.notes.filter((existing) => existing.id !== note.id);
      const draft = source.drafts.get(note.id);
      if (!draft) continue;
      source.drafts.delete(note.id);
      if (!retained.has(draft)) {
        retained.set(draft, readingNoteDirty(draft) ? {
          ...draft, id: crypto.randomUUID(), note: null, savedBody: "", saving: false, error: null,
          notice: "Saved note deleted. Your unsaved edits remain as a new draft.",
        } : null);
      }
      const kept = retained.get(draft);
      if (kept) source.drafts.set(kept.id, kept);
      if (source.activeDraftId === note.id) source.activeDraftId = kept?.id ?? null;
    }
  }
  updateReadingNotes();
}

/** The confirmation owns this snapshot; never expand it to include newly arriving notes. */
export async function deleteReadingNotes(
  source: ReadingNotesSource, notes: ReadingNote[], wholePage = false,
): Promise<ReadingNotesDeleteResult> {
  const { graphPath, pageId } = source;
  const destinations = [...sources.values()].filter((item) => item.graphPath === graphPath);
  if (destinations.some((item) => item.deleting || [...item.drafts.values()].some((draft) => draft.saving))) {
    throw new Error("Wait for the current note operation to finish, then try deleting again.");
  }
  if (!notes.length) throw new Error("There are no saved notes to delete.");
  if (wholePage && (!pageId || notes.some((note) => note.source.pageId !== pageId))) {
    throw new Error("Delete all is limited to notes on the current page.");
  }
  if (!wholePage && notes.length !== 1) throw new Error("Choose one saved note to delete.");
  const snapshot = notes.map((note) => ({ ...note }));
  const affectedPages = [...new Set(snapshot.map((note) =>
    note.storage === "inline" ? note.source.pageId ?? "" : note.notePageId).filter(Boolean))];
  for (const destination of destinations) destination.deleting = true;
  updateReadingNotes();
  try {
    return await withNoteSourceEditors(graphPath, affectedPages, async () => {
      const receipt = wholePage
        ? await readingNotesDeleteForPage(graphPath, pageId, snapshot.map((note) => ({ id: note.id, revision: note.revision })))
        : await readingNoteDelete(graphPath, snapshot[0].id, snapshot[0].revision);
      const deleted = new Set(receipt.deletedIds);
      publishDeletedNotes(graphPath, snapshot.filter((note) => deleted.has(note.id)));
      const failures = receipt.failures.map((failure) => failure.message);
      try {
        if ((await getGraphInfo()).path === graphPath) await Promise.all(affectedPages.map((id) => reloadPageEditors(id)));
      } catch (error) {
        affectedPages.forEach((id) => markPageEditorsStale(id));
        failures.push(`The source editor could not refresh. It is protected from stale edits. Reopen the page. ${errorText(error)}`);
      }
      if (failures.length) throw new Error(
        `${receipt.deletedCount} of ${snapshot.length} saved notes deleted, but the operation needs attention: ${failures.join(" ")}`
        + (receipt.backups.length ? " Recovery copies are kept beside the affected source files." : ""),
      );
      return receipt;
    });
  } finally {
    for (const destination of sources.values()) {
      if (destination.graphPath === graphPath) destination.deleting = false;
    }
    updateReadingNotes();
  }
}
