import { writable } from "svelte/store";
import { getGraphInfo } from "./api";
import {
  bookNoteSave, bookNoteDelete, bookNoteResolve, bookNotesList, notifyBookNotesChanged, selectionForBook, hasBookNoteConflicts,
  type BookInfo, type BookNote, type BookNoteConflict, type BookLocation, type BookSelection,
} from "./books";

export interface BookNoteDraft {
  id: string; body: string; quote: string; locator: BookLocation | null; sourceSha256: string;
  note: BookNote | null; saved: string; saving: boolean; error: string; notice: string;
  attachmentChosen: boolean;
}
export interface BookNotesSource {
  graphPath: string; book: BookInfo; notes: BookNote[]; drafts: Map<string, BookNoteDraft>;
  activeId: string | null; loading: boolean; error: string; generation: number;
}
const sources = new Map<string, BookNotesSource>();
export const bookDraftChanges = writable(0);
export const updateBookDrafts = () => bookDraftChanges.update(n => n + 1);
const signature = (d: Pick<BookNoteDraft, "body" | "quote" | "locator" | "sourceSha256">) =>
  JSON.stringify([d.body, d.quote, d.locator, d.sourceSha256]);
export const bookDraftDirty = (draft: BookNoteDraft) => signature(draft) !== draft.saved;

export function getBookNotesSource(graphPath: string, book: BookInfo): BookNotesSource {
  const key = JSON.stringify([graphPath, book.id]);
  let source = sources.get(key);
  if (!source) {
    source = { graphPath, book, notes: [], drafts: new Map(), activeId: null, loading: false, error: "", generation: 0 };
    sources.set(key, source);
  }
  source.book = book;
  return source;
}
export function newBookNoteDraft(source: BookNotesSource): BookNoteDraft {
  const draft: BookNoteDraft = {
    id: crypto.randomUUID(), body: "", quote: "", locator: null, sourceSha256: source.book.sourceSha256,
    note: null, saved: "", saving: false, error: "", notice: "", attachmentChosen: true,
  };
  draft.saved = signature(draft);
  source.drafts.set(draft.id, draft); source.activeId = draft.id;
  updateBookDrafts(); return draft;
}
export function editBookNote(source: BookNotesSource, note: BookNote): BookNoteDraft {
  let draft = source.drafts.get(note.id);
  if (!draft) {
    const conflicted = hasBookNoteConflicts(note);
    draft = { id: note.id, body: conflicted ? "" : note.body, quote: conflicted ? "" : note.quote,
      locator: conflicted ? null : structuredClone(note.locator),
      sourceSha256: note.sourceSha256, note, saved: "", saving: false, error: "", notice: "",
      attachmentChosen: !conflicted };
    draft.saved = signature(draft);
    source.drafts.set(draft.id, draft);
  } else if (!draft.saving && !bookDraftDirty(draft) && !hasBookNoteConflicts(note) && !hasBookNoteConflicts(draft.note)) {
    Object.assign(draft, { note, body: note.body, quote: note.quote, locator: structuredClone(note.locator), sourceSha256: note.sourceSha256 });
    draft.saved = signature(draft);
  }
  source.activeId = draft.id; updateBookDrafts(); return draft;
}
function selectResolutionRevision(source: BookNotesSource, draft: BookNoteDraft) {
  const latest = source.notes.find(n => n.id === draft.id);
  if (hasBookNoteConflicts(latest)) draft.note = latest!;
  draft.attachmentChosen = true;
}
export function chooseBookNoteCandidate(source: BookNotesSource, note: BookNote, candidate: BookNoteConflict, includeBody = true): void {
  const current = source.notes.find(n => n.id === note.id);
  if (current?.revision !== note.revision || !current.conflicts?.some(c => c.revision === candidate.revision)) return;
  const draft = editBookNote(source, note);
  if (draft.saving || (candidate.deleted && includeBody)) return;
  selectResolutionRevision(source, draft);
  if (includeBody) draft.body = candidate.body;
  draft.quote = candidate.quote; draft.locator = structuredClone(candidate.locator); draft.sourceSha256 = candidate.sourceSha256;
  draft.error = ""; draft.notice = "Candidate selected for review. Edit the merged Markdown, then explicitly resolve.";
  updateBookDrafts();
}
export function useBookSelection(source: BookNotesSource, draft: BookNoteDraft, value: BookSelection | null): void {
  if (draft.saving) return;
  const selection = selectionForBook(value, source.graphPath, source.book);
  if (!selection) draft.error = "Select a passage in this original book first. Selections from another graph or source cannot be used.";
  else {
    selectResolutionRevision(source, draft);
    draft.quote = selection.quote; draft.locator = selection.locator; draft.sourceSha256 = selection.sourceSha256;
    draft.error = ""; draft.notice = "";
  }
  updateBookDrafts();
}
export function clearBookPassage(source: BookNotesSource, draft: BookNoteDraft): void {
  if (draft.saving) return;
  if (source.book.sourceAvailable === false) {
    draft.error = "The original is unavailable. Keep an existing note or candidate attachment until the source is restored.";
    updateBookDrafts(); return;
  }
  selectResolutionRevision(source, draft);
  draft.quote = ""; draft.locator = null; draft.sourceSha256 = source.book.sourceSha256; draft.notice = ""; updateBookDrafts();
}
export async function loadBookNotes(source: BookNotesSource): Promise<void> {
  const generation = ++source.generation;
  source.loading = true; source.error = ""; updateBookDrafts();
  try {
    const notes = await bookNotesList(source.graphPath, source.book.id);
    if (generation === source.generation) { source.notes = notes; notifyBookNotesChanged(); }
  } catch (e) {
    if (generation === source.generation) source.error = `Could not load notes. Drafts are kept. ${String(e)}`;
  } finally {
    if (generation === source.generation) { source.loading = false; updateBookDrafts(); }
  }
}
async function assertGraph(source: BookNotesSource) {
  if ((await getGraphInfo()).path !== source.graphPath)
    throw new Error("The graph changed. Return to this draft's original graph before saving.");
}
export async function saveBookNoteDraft(source: BookNotesSource, draft: BookNoteDraft): Promise<void> {
  if (draft.saving) return;
  if (source.book.sourceAvailable === false && !draft.note) {
    draft.error = "The original is unavailable. Existing notes remain editable, but new notes require the source.";
    updateBookDrafts(); return;
  }
  if (hasBookNoteConflicts(draft.note) || hasBookNoteConflicts(source.notes.find(n => n.id === draft.id))) {
    draft.error = "Review every conflict candidate and use Resolve with merged note, or confirm deletion.";
    updateBookDrafts(); return;
  }
  if (!draft.body.trim()) { draft.error = "Write a note before saving."; updateBookDrafts(); return; }
  const args = { graphPath: source.graphPath, bookId: source.book.id, noteId: draft.id,
    expectedRevision: draft.note?.revision ?? null, body: draft.body, quote: draft.quote,
    locator: structuredClone(draft.locator), sourceSha256: draft.sourceSha256 };
  draft.saving = true; draft.error = ""; draft.notice = ""; updateBookDrafts();
  try {
    await assertGraph(source);
    const unchangedSavedAnchor = draft.note && args.sourceSha256 === draft.note.sourceSha256
      && args.quote === draft.note.quote && JSON.stringify(args.locator) === JSON.stringify(draft.note.locator);
    if (args.sourceSha256 !== source.book.sourceSha256 && !unchangedSavedAnchor)
      throw new Error("The original file changed. Explicitly use a selection from the current book or choose Whole-book note to reattach.");
    const note = await bookNoteSave(args);
    ++source.generation; source.loading = false;
    source.notes = [note, ...source.notes.filter(n => n.id !== note.id)];
    draft.note = note;
    draft.saved = signature({ body: note.body, quote: note.quote, locator: note.locator, sourceSha256: note.sourceSha256 });
    draft.notice = bookDraftDirty(draft) ? "Saved. Newer edits are still unsaved."
      : note.filePath.endsWith(".jsonld") ? "Saved in adjacent JSON-LD; original book unchanged." : "Saved in legacy Markdown; original book unchanged.";
    notifyBookNotesChanged();
  } catch (e) {
    draft.error = `Could not save. Your draft is kept in this session. ${String(e)}`;
    if (/revision|conflict|annotation changed/i.test(String(e))) {
      draft.error += " Refresh and review the saved version before retrying.";
      await loadBookNotes(source);
    }
  } finally { draft.saving = false; updateBookDrafts(); }
}
export function reviewBookNoteRevision(source: BookNotesSource, draft: BookNoteDraft, revision: string): void {
  const note = source.notes.find(n => n.id === draft.id && n.revision === revision);
  if (!note) draft.error = "The saved version changed again. Refresh and review it.";
  else if (!draft.saving) {
    const changed = draft.note?.revision !== note.revision;
    draft.note = note; draft.saved = signature(note);
    if (changed && hasBookNoteConflicts(note)) draft.attachmentChosen = false;
    draft.error = ""; draft.notice = "Reviewed revision selected. Your draft is unchanged; Save note retries explicitly.";
  }
  updateBookDrafts();
}
export async function resolveBookNoteDraft(source: BookNotesSource, draft: BookNoteDraft, remove = false): Promise<void> {
  if (draft.saving) return;
  const note = source.notes.find(n => n.id === draft.id);
  if (!note || !hasBookNoteConflicts(note) || note.revision !== draft.note?.revision) {
    draft.error = "The conflict changed. Review the current candidates and explicitly select an attachment again.";
    updateBookDrafts(); return;
  }
  if (!remove && (!draft.body.trim() || !draft.attachmentChosen)) {
    draft.error = "Write the merged note and explicitly choose a candidate attachment, a current selection, or Whole-book note.";
    updateBookDrafts(); return;
  }
  const sourceUnavailable = source.book.sourceAvailable === false;
  if (remove && sourceUnavailable && !draft.attachmentChosen) {
    draft.error = "The original is unavailable. Explicitly choose a candidate attachment before confirming deletion.";
    updateBookDrafts(); return;
  }
  const keepAttachment = !remove || sourceUnavailable;
  const args = { graphPath: source.graphPath, bookId: source.book.id, noteId: draft.id,
    expectedRevision: note.revision, body: remove ? "" : draft.body, quote: keepAttachment ? draft.quote : "",
    locator: keepAttachment ? structuredClone(draft.locator) : null,
    sourceSha256: keepAttachment ? draft.sourceSha256 : source.book.sourceSha256, delete: remove };
  const hadAttachment = draft.attachmentChosen;
  draft.saving = true; draft.error = ""; draft.notice = ""; updateBookDrafts();
  try {
    await assertGraph(source);
    await bookNoteResolve(args);
    // Only a successful, fenced resolution may discard the displayed candidates.
    source.notes = source.notes.filter(n => n.id !== draft.id);
    draft.attachmentChosen = true;
    if (remove) {
      draft.note = null;
      if (!hadAttachment) {
        draft.quote = ""; draft.locator = null; draft.sourceSha256 = source.book.sourceSha256;
      }
      source.drafts.delete(draft.id);
      draft.id = crypto.randomUUID(); draft.saved = "";
      source.drafts.set(draft.id, draft); source.activeId = draft.id;
      draft.notice = "Deletion resolved. Your composer text is kept as a new unsaved draft.";
    } else {
      draft.saved = signature(args);
      draft.notice = "Conflict resolved with your merged note. Original book unchanged.";
    }
    notifyBookNotesChanged();
    await loadBookNotes(source);
    if (!remove) {
      const saved = source.notes.find(n => n.id === args.noteId);
      if (saved) draft.note = saved;
    }
  } catch (e) {
    draft.error = `Could not resolve. Your merged draft and candidates are kept. ${String(e)}`;
    draft.attachmentChosen = false;
    await loadBookNotes(source);
  } finally { draft.saving = false; updateBookDrafts(); }
}
export async function deleteBookNote(source: BookNotesSource, note: BookNote): Promise<void> {
  if (hasBookNoteConflicts(note) || hasBookNoteConflicts(source.notes.find(n => n.id === note.id)))
    throw new Error("Conflicted notes require explicit resolution; review all candidates before confirming deletion.");
  await assertGraph(source);
  await bookNoteDelete(source.graphPath, source.book.id, note.id, note.revision);
  ++source.generation; source.loading = false;
  source.notes = source.notes.filter(n => n.id !== note.id);
  const draft = source.drafts.get(note.id);
  if (draft) {
    source.drafts.delete(note.id);
    if (bookDraftDirty(draft)) {
      draft.id = crypto.randomUUID(); draft.note = null; draft.saved = "";
      draft.notice = "Saved note removed. Your unsaved edits remain as a new draft.";
      source.drafts.set(draft.id, draft); source.activeId = draft.id;
    } else if (source.activeId === note.id) source.activeId = null;
  }
  notifyBookNotesChanged(); updateBookDrafts();
}
