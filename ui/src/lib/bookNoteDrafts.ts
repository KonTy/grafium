import { writable } from "svelte/store";
import { getGraphInfo } from "./api";
import {
  bookNoteSave, bookNoteDelete, bookNotesList, notifyBookNotesChanged, selectionForBook,
  type BookInfo, type BookNote, type BookLocation, type BookSelection,
} from "./books";

export interface BookNoteDraft {
  id: string; body: string; quote: string; locator: BookLocation | null; sourceSha256: string;
  note: BookNote | null; saved: string; saving: boolean; error: string; notice: string;
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
    note: null, saved: "", saving: false, error: "", notice: "",
  };
  draft.saved = signature(draft);
  source.drafts.set(draft.id, draft); source.activeId = draft.id;
  updateBookDrafts(); return draft;
}
export function editBookNote(source: BookNotesSource, note: BookNote): BookNoteDraft {
  let draft = source.drafts.get(note.id);
  if (!draft) {
    draft = { id: note.id, body: note.body, quote: note.quote, locator: structuredClone(note.locator),
      sourceSha256: note.sourceSha256, note, saved: "", saving: false, error: "", notice: "" };
    draft.saved = signature(draft);
    source.drafts.set(draft.id, draft);
  } else if (!draft.saving && !bookDraftDirty(draft)) {
    Object.assign(draft, { note, body: note.body, quote: note.quote, locator: structuredClone(note.locator), sourceSha256: note.sourceSha256 });
    draft.saved = signature(draft);
  }
  source.activeId = draft.id; updateBookDrafts(); return draft;
}
export function useBookSelection(source: BookNotesSource, draft: BookNoteDraft, value: BookSelection | null): void {
  if (draft.saving) return;
  const selection = selectionForBook(value, source.graphPath, source.book);
  if (!selection) draft.error = "Select a passage in this original book first. Selections from another graph or source cannot be used.";
  else {
    draft.quote = selection.quote; draft.locator = selection.locator; draft.sourceSha256 = selection.sourceSha256;
    draft.error = ""; draft.notice = "";
  }
  updateBookDrafts();
}
export function clearBookPassage(source: BookNotesSource, draft: BookNoteDraft): void {
  if (draft.saving) return;
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
    draft.notice = bookDraftDirty(draft) ? "Saved. Newer edits are still unsaved." : "Saved in separate graph Markdown; original book unchanged.";
    notifyBookNotesChanged();
  } catch (e) {
    draft.error = `Could not save. Your draft is kept in this session. ${String(e)}`;
    if (/revision|conflict/i.test(String(e))) draft.error += " Refresh and review the saved version before retrying.";
  } finally { draft.saving = false; updateBookDrafts(); }
}
export function reviewBookNoteRevision(source: BookNotesSource, draft: BookNoteDraft, revision: string): void {
  const note = source.notes.find(n => n.id === draft.id && n.revision === revision);
  if (!note) draft.error = "The saved version changed again. Refresh and review it.";
  else if (!draft.saving) {
    draft.note = note; draft.saved = signature(note);
    draft.error = ""; draft.notice = "Reviewed revision selected. Your draft is unchanged; Save note retries explicitly.";
  }
  updateBookDrafts();
}
export async function deleteBookNote(source: BookNotesSource, note: BookNote): Promise<void> {
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
