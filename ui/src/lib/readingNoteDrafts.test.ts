import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getReadingNotesSource, newReadingNoteDraft, editReadingNote, readingNoteDirty, loadReadingNotes,
  saveReadingNoteDraft, reattachReadingNoteDraft,
  focusReadingNoteLabel, applyReadingNoteFocusRequest, useReviewedReadingNoteRevision,
} from "./readingNoteDrafts";
import { sourceReadingSelection } from "./readingSelection";
import type { ReadingNote } from "./readingNotes";

const mocks = vi.hoisted(() => ({ graph: vi.fn(), flush: vi.fn(), reload: vi.fn(), lock: vi.fn(), stale: vi.fn(), invoke: vi.fn() }));
vi.mock("./api", () => ({ getGraphInfo: mocks.graph }));
vi.mock("./editorPersistence", () => ({
  flushPageEditors: mocks.flush, reloadPageEditors: mocks.reload, withPageEditorsLocked: mocks.lock,
  markPageEditorsStale: mocks.stale,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

let count = 0;
let graphPath: string;
let storedNotes: Map<string, ReadingNote>;
function savedNote(overrides: Partial<ReadingNote> = {}): ReadingNote {
  return {
    id: "note-1", notePageId: "note-page-1", filePath: "pages/Reading notes/one.md", body: "Saved body",
    revision: "r1", source: { pageId: "a", pageTitle: "A", filePath: "pages/A.md" },
    quote: "Quoted passage", status: "attached", statusMessage: "Attached", targetBlockId: "block-a",
    createdAt: "2026-09-15T00:00:00Z", updatedAt: "2026-09-15T00:00:00Z",
    storage: "file", footnoteLabel: null, noteBlockId: null, ...overrides,
  };
}
function selection(pageId = "a") { return sourceReadingSelection(pageId, `block-${pageId}`, "Before quote after", 7, 12)!; }
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  vi.resetAllMocks();
  graphPath = `/synthetic/notes-${++count}`;
  mocks.graph.mockResolvedValue({ path: graphPath });
  mocks.flush.mockResolvedValue(undefined);
  mocks.reload.mockResolvedValue(undefined);
  mocks.lock.mockImplementation(async (_id, operation) => operation());
  storedNotes = new Map();
  mocks.invoke.mockImplementation(async (command, args) => {
    if (command === "reading_notes_list") return {
      notes: [...storedNotes.values()].reverse().filter((note) => !args.pageId || note.source.pageId === args.pageId), warnings: [],
    };
    const note = savedNote({ id: args.noteId, body: args.body ?? "Saved body", revision: "r2" });
    storedNotes.delete(note.id);
    storedNotes.set(note.id, note);
    return note;
  });
});

describe("session-owned reading note drafts", () => {
  it("treats revisions as opaque and requires explicit review of a changed note after Refresh", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const old = savedNote({ id: "other-note", storage: "inline", revision: "other-note-r1" });
    source.notes = [old];
    const otherDraft = editReadingNote(source, old);
    otherDraft.body = "Keep my unsaved edit";
    const draft = newReadingNoteDraft(source);
    draft.body = "Another footnote";
    const refreshed = { ...old, revision: "other-note-r2", body: "Externally edited note" };
    const created = savedNote({ id: draft.id, body: draft.body, storage: "inline", revision: "new-note-r1" });
    mocks.invoke.mockImplementation(async (command) => command === "reading_notes_list"
      ? { notes: [created, refreshed], warnings: [] } : created);
    await saveReadingNoteDraft(source, draft);
    expect(mocks.invoke.mock.calls.filter((call) => call[0] === "reading_notes_list")).toEqual([]);
    await loadReadingNotes(source);
    expect(source.notes.find((note) => note.id === old.id)?.revision).toBe("other-note-r2");
    expect(otherDraft.note?.revision).toBe("other-note-r1");
    expect(otherDraft.body).toBe("Keep my unsaved edit");
    useReviewedReadingNoteRevision(source, otherDraft, "other-note-r2");
    expect(otherDraft.note?.revision).toBe("other-note-r2");
    expect(otherDraft.body).toBe("Keep my unsaved edit");
    expect(otherDraft.notice).toContain("choose Save note to retry");
    expect(mocks.invoke.mock.calls.filter((call) => call[0] === "reading_note_update")).toEqual([]);
    source.notes = [{ ...refreshed, revision: "other-note-r3", body: "External body edit" }];
    useReviewedReadingNoteRevision(source, otherDraft, "other-note-r2");
    expect(otherDraft.note?.revision).toBe("other-note-r2");
    expect(otherDraft.error).toContain("changed again");
  });

  it("keeps a newer unsaved create draft when its previously unacknowledged file is discovered", () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source);
    draft.body = "Newer body after a failed acknowledgement";
    const recovered = savedNote({ id: draft.id, body: "First body already written" });
    expect(editReadingNote(source, recovered)).toBe(draft);
    expect(draft.body).toBe("Newer body after a failed acknowledgement");
    expect(draft.note).toBeNull();
    source.notes = [recovered];
    useReviewedReadingNoteRevision(source, draft, recovered.revision);
    expect(draft.note).toBe(recovered);
    expect(draft.body).toBe("Newer body after a failed acknowledgement");
  });

  it("processes a marker trigger once across pane remounts without abandoning the active draft", () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    source.notes = [savedNote({ storage: "inline", footnoteLabel: "grafium-note-1" })];
    applyReadingNoteFocusRequest(source, "a", "grafium-note-1", 1);
    expect(source.activeDraftId).toBe("note-1");
    const draft = newReadingNoteDraft(source);
    draft.body = "New unfinished thought";
    const remounted = getReadingNotesSource(graphPath, "a", "A");
    applyReadingNoteFocusRequest(remounted, "a", "grafium-note-1", 1);
    expect(source.activeDraftId).toBe(draft.id);
    applyReadingNoteFocusRequest(remounted, "b", "grafium-note-1", 2);
    expect(source.activeDraftId).toBe(draft.id);
    applyReadingNoteFocusRequest(remounted, "a", "grafium-note-1", 2);
    expect(source.activeDraftId).toBe("note-1");
    expect(source.drafts.get(draft.id)?.body).toBe("New unfinished thought");
  });

  it("keeps missing-marker errors across remounts without creating a note", () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    applyReadingNoteFocusRequest(source, "a", "grafium-note-1", 1);
    expect(source.focusError).toContain("No note was created");
    expect(source.drafts.size).toBe(0);
    applyReadingNoteFocusRequest(getReadingNotesSource(graphPath, "a", "A"), "a", "grafium-note-1", 1);
    expect(source.focusError).toContain("No note was created");
    expect(source.drafts.size).toBe(0);
  });

  it("selects a requested footnote only after it exists in the loaded list, keeping dirty drafts", () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const unfinished = newReadingNoteDraft(source);
    unfinished.body = "Keep my work";
    expect(() => focusReadingNoteLabel(source, "a", "grafium-note-1")).toThrow("No note was created");
    expect(source.drafts.size).toBe(1);
    expect(source.activeDraftId).toBe(unfinished.id);
    source.notes = [savedNote({ storage: "inline", footnoteLabel: "grafium-note-1", noteBlockId: "footer" })];
    const editing = focusReadingNoteLabel(source, "a", "grafium-note-1");
    expect(source.activeDraftId).toBe(editing.id);
    expect(source.drafts.get(unfinished.id)?.body).toBe("Keep my work");
    expect(() => focusReadingNoteLabel(source, "b", "grafium-note-1")).toThrow("not found");
  });

  it("keeps inline sources locked through native writes and awaited reloads", async () => {
    const order: string[] = [];
    mocks.lock.mockImplementation(async (id, operation) => {
      order.push(`lock:${id}`);
      try { return await operation(); } finally { order.push(`unlock:${id}`); }
    });
    mocks.flush.mockImplementation(async (id) => { order.push(`flush:${id}`); });
    mocks.reload.mockImplementation(async (id) => { order.push(`reload:${id}`); });
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "reading_notes_list") return { notes: [...storedNotes.values()], warnings: [] };
      order.push("write");
      const note = savedNote({ id: args.noteId, body: args.body, storage: "inline", footnoteLabel: "grafium-note-1" });
      storedNotes.set(note.id, note);
      return note;
    });
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source);
    draft.body = "Inline note";
    await saveReadingNoteDraft(source, draft);
    expect(order).toEqual(["lock:a", "flush:a", "write", "reload:a", "unlock:a"]);
    expect(draft.note?.storage).toBe("inline");
    expect(readingNoteDirty(draft)).toBe(false);
    order.length = 0;
    draft.body = "Edited note";
    await saveReadingNoteDraft(source, draft);
    expect(order).toEqual(["lock:a", "flush:a", "write", "reload:a", "unlock:a"]);
  });

  it("retains the successful receipt and newer draft when a source reload fails", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source);
    draft.body = "Persist this";
    mocks.reload.mockImplementation(async () => {
      draft.body = "Newer unsaved thought";
      throw new Error("Source read failed");
    });
    await saveReadingNoteDraft(source, draft);
    expect(draft.note?.body).toBe("Persist this");
    expect(draft.error).toContain("Note saved, but");
    expect(draft.error).toContain("Source read failed");
    expect(draft.body).toBe("Newer unsaved thought");
    expect(readingNoteDirty(draft)).toBe(true);
  });

  it("protects stale source editors when graph verification fails after the file was written", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source);
    draft.body = "Persist first";
    mocks.graph.mockResolvedValueOnce({ path: graphPath })
      .mockResolvedValueOnce({ path: graphPath })
      .mockResolvedValueOnce({ path: graphPath })
      .mockRejectedValueOnce(new Error("Graph lookup failed"));
    await saveReadingNoteDraft(source, draft);
    expect(draft.note?.body).toBe("Persist first");
    expect(mocks.stale).toHaveBeenCalledWith("a");
    expect(draft.error).toContain("Graph lookup failed");
    expect(draft.error).toContain("Note saved");
  });

  it("isolates graphs and sources and restores every draft across tab/pane remounts", () => {
    const a = getReadingNotesSource(graphPath, "a", "A");
    const first = newReadingNoteDraft(a, selection());
    first.body = "Unfinished first thought";
    const second = newReadingNoteDraft(a);
    second.body = "Another thought";
    const b = getReadingNotesSource(graphPath, "b", "B");
    const otherGraph = getReadingNotesSource(`${graphPath}-other`, "a", "A");
    expect(b.drafts.size).toBe(0);
    expect(otherGraph.drafts.size).toBe(0);
    expect(getReadingNotesSource(graphPath, "a", "Renamed")).toBe(a);
    expect(a.pageTitle).toBe("Renamed");
    expect(a.activeDraftId).toBe(second.id);
    expect(a.drafts.get(first.id)?.body).toBe("Unfinished first thought");
    expect(a.drafts.get(first.id)?.selection?.text).toBe("quote");
  });

  it("uses a real stable UUID per new note and reuses it on failed-create retry", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source, selection());
    draft.body = "Keep me";
    expect(draft.id).toMatch(/^[\da-f]{8}-[\da-f]{4}-4[\da-f]{3}-[89ab][\da-f]{3}-[\da-f]{12}$/i);
    mocks.invoke.mockRejectedValueOnce("Disk unavailable");
    await saveReadingNoteDraft(source, draft);
    expect(draft.body).toBe("Keep me");
    expect(draft.selection?.text).toBe("quote");
    expect(draft.error).toContain("Disk unavailable");
    expect(readingNoteDirty(draft)).toBe(true);
    await saveReadingNoteDraft(source, draft);
    const ids = mocks.invoke.mock.calls.filter((call) => call[0] === "reading_note_create").map((call) => call[1].noteId);
    expect(ids).toEqual([draft.id, draft.id]);
    expect(draft.error).toBeNull();
    expect(draft.selection).toBeNull();
    expect(readingNoteDirty(draft)).toBe(false);
    expect(draft.notice).toContain("Saved");
  });

  it("pins source, UUID, body and full quote before awaits, preserving newer body edits on navigation", async () => {
    const waiting = deferred<void>();
    mocks.flush.mockReturnValue(waiting.promise);
    const a = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(a, selection());
    draft.body = "Original body";
    const pending = saveReadingNoteDraft(a, draft);
    await vi.waitFor(() => expect(mocks.flush).toHaveBeenCalledWith("a"));
    const b = getReadingNotesSource(graphPath, "b", "B");
    newReadingNoteDraft(b).body = "B's private draft";
    draft.selection!.parts[0].text = "New selection";
    draft.body = "More typing during save";
    waiting.resolve();
    await pending;
    expect(mocks.invoke).toHaveBeenCalledWith("reading_note_create", expect.objectContaining({
      graphPath, noteId: draft.id, sourcePageId: "a", body: "Original body",
      selection: expect.objectContaining({ parts: [expect.objectContaining({ text: "quote" })] }),
    }));
    expect(draft.body).toBe("More typing during save");
    expect(draft.savedBody).toBe("Original body");
    expect(readingNoteDirty(draft)).toBe(true);
    expect(draft.notice).toContain("Newer edits");
    expect(b.drafts.get(b.activeDraftId!)?.body).toBe("B's private draft");
    expect(b.notes).toEqual([]);
    expect(a.notes[0].body).toBe("Original body");
  });

  it("never flushes editors in the wrong graph, including after a graph-info delay", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source, selection());
    draft.body = "Keep me";
    mocks.graph.mockResolvedValue({ path: "other" });
    await saveReadingNoteDraft(source, draft);
    expect(mocks.flush).not.toHaveBeenCalled();
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(draft.error).toContain("graph changed");
    expect(draft.body).toBe("Keep me");
  });

  it("rechecks the graph after flushing and does not create in a switched graph", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source, selection());
    draft.body = "Keep me";
    mocks.graph.mockResolvedValueOnce({ path: graphPath }).mockResolvedValueOnce({ path: graphPath }).mockResolvedValueOnce({ path: "other" });
    await saveReadingNoteDraft(source, draft);
    expect(mocks.flush).toHaveBeenCalledTimes(1);
    expect(mocks.flush).toHaveBeenCalledWith("a");
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(draft.body).toBe("Keep me");
    expect(draft.selection?.text).toBe("quote");
  });

  it("settles a successful response into the original source after the graph changes", async () => {
    const waiting = deferred<ReadingNote>();
    mocks.invoke.mockReturnValue(waiting.promise);
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source);
    draft.body = "Saved in A";
    const pending = saveReadingNoteDraft(source, draft);
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalled());
    const other = getReadingNotesSource("other-graph", "a", "A");
    mocks.graph.mockResolvedValue({ path: "other-graph" });
    waiting.resolve(savedNote({ id: draft.id, body: draft.body }));
    await pending;
    expect(draft.note?.body).toBe("Saved in A");
    expect(readingNoteDirty(draft)).toBe(false);
    expect(other.notes).toEqual([]);
    expect(other.drafts.size).toBe(0);
  });

  it("preserves source-editor flush errors and prevents duplicate in-flight saves", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const draft = newReadingNoteDraft(source);
    draft.body = "Draft";
    mocks.flush.mockRejectedValue(new Error("Source could not be saved"));
    await Promise.all([saveReadingNoteDraft(source, draft), saveReadingNoteDraft(source, draft)]);
    expect(mocks.flush).toHaveBeenCalledTimes(1);
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(draft.error).toContain("Source could not be saved");
    expect(draft.body).toBe("Draft");
  });

  it("uses revision guards and retains conflicting edits when reselected in another scope", async () => {
    const a = getReadingNotesSource(graphPath, "a", "A");
    const draft = editReadingNote(a, savedNote());
    draft.body = "My edits";
    mocks.invoke.mockRejectedValueOnce({ code: "revision_conflict", message: "The Markdown file changed. Open note to review it." });
    await saveReadingNoteDraft(a, draft);
    expect(mocks.invoke).toHaveBeenCalledWith("reading_note_update", {
      graphPath, noteId: "note-1", expectedRevision: "r1", body: "My edits",
    });
    expect(mocks.flush).toHaveBeenCalledWith("note-page-1");
    expect(draft.body).toBe("My edits");
    expect(draft.note?.revision).toBe("r1");
    expect(draft.error).toContain("Markdown file changed");
    const b = getReadingNotesSource(graphPath, "b", "B");
    expect(editReadingNote(b, savedNote({ revision: "external-r2", body: "External edit" }))).toBe(draft);
    expect(draft.body).toBe("My edits");
    expect(draft.note?.revision).toBe("r1");
  });

  it("reuses the revised receipt for later saves and keeps the first new draft", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const newDraft = newReadingNoteDraft(source);
    newDraft.body = "Unfinished";
    const draft = editReadingNote(source, savedNote());
    draft.body = "Edit 1";
    await saveReadingNoteDraft(source, draft);
    draft.body = "Edit 2";
    await saveReadingNoteDraft(source, draft);
    expect(mocks.invoke.mock.calls.findLast((call) => call[0] === "reading_note_update")?.[1]).toMatchObject({ expectedRevision: "r2", body: "Edit 2" });
    expect(source.drafts.get(newDraft.id)?.body).toBe("Unfinished");
  });

  it("reattaches only an explicit current-page selection and preserves unsaved body changes", async () => {
    const source = getReadingNotesSource(graphPath, "b", "B");
    const draft = editReadingNote(source, savedNote({ status: "orphaned" }));
    draft.body = "Unsaved rewrite";
    draft.selection = selection("a");
    await reattachReadingNoteDraft(source, draft, "b");
    expect(mocks.invoke).not.toHaveBeenCalled();
    draft.selection = selection("b");
    await reattachReadingNoteDraft(source, draft, "b");
    expect(mocks.flush).toHaveBeenCalledTimes(2);
    expect(mocks.flush).toHaveBeenCalledWith("b");
    expect(mocks.invoke).toHaveBeenCalledWith("reading_note_reattach", {
      graphPath, noteId: "note-1", expectedRevision: "r1", sourcePageId: "b", selection: selection("b"),
    });
    expect(draft.body).toBe("Unsaved rewrite");
    expect(draft.savedBody).toBe("Saved body");
    expect(draft.selection).toBeNull();
    expect(draft.note?.revision).toBe("r2");
    expect(draft.notice).toContain("Body edits are still unsaved");
    await saveReadingNoteDraft(source, draft);
    expect(mocks.invoke.mock.lastCall?.[1]).toMatchObject({ expectedRevision: "r2", body: "Unsaved rewrite" });
  });

  it("keeps selected quote and body when reattachment fails or the graph differs", async () => {
    const source = getReadingNotesSource(graphPath, "b", "B");
    const draft = editReadingNote(source, savedNote({ status: "ambiguous" }));
    draft.selection = selection("b");
    draft.body = "Do not lose";
    mocks.invoke.mockRejectedValueOnce("Revision conflict");
    await reattachReadingNoteDraft(source, draft, "b");
    expect(draft.body).toBe("Do not lose");
    expect(draft.selection?.pageId).toBe("b");
    expect(draft.error).toContain("Revision conflict");
    mocks.flush.mockClear();
    mocks.invoke.mockClear();
    mocks.graph.mockResolvedValue({ path: "other-graph" });
    await reattachReadingNoteDraft(source, draft, "b");
    expect(mocks.flush).not.toHaveBeenCalled();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});

describe("reading note lists", () => {
  it("shows warnings and orphan notes from All notes without requiring a source page", async () => {
    const source = getReadingNotesSource(graphPath, "", "");
    const orphan = savedNote({ status: "orphaned", source: { pageId: null, pageTitle: "Gone", filePath: "pages/Gone.md" } });
    mocks.invoke.mockResolvedValue({ notes: [orphan], warnings: ["Skipped invalid note: broken.md"] });
    await loadReadingNotes(source);
    expect(source.scope).toBe("all");
    expect(mocks.invoke).toHaveBeenCalledWith("reading_notes_list", { graphPath });
    expect(source.notes).toEqual([orphan]);
    expect(source.warnings).toEqual(["Skipped invalid note: broken.md"]);
  });

  it("ignores stale responses after changing the list scope", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const waiting = deferred<{ notes: ReadingNote[]; warnings: string[] }>();
    mocks.invoke.mockReturnValueOnce(waiting.promise);
    const pending = loadReadingNotes(source);
    source.scope = "all";
    mocks.invoke.mockResolvedValueOnce({ notes: [savedNote({ id: "all-note" })], warnings: ["All warning"] });
    await loadReadingNotes(source);
    waiting.resolve({ notes: [], warnings: [] });
    await pending;
    expect(source.notes[0].id).toBe("all-note");
    expect(source.warnings).toEqual(["All warning"]);
    expect(source.loading).toBe(false);
  });

  it("keeps completed saves alongside unrelated list results arriving later", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    const waiting = deferred<{ notes: ReadingNote[]; warnings: string[] }>();
    storedNotes.set("older", savedNote({ id: "older" }));
    mocks.invoke.mockReturnValueOnce(waiting.promise);
    const pending = loadReadingNotes(source);
    const draft = newReadingNoteDraft(source);
    draft.body = "Just saved";
    await saveReadingNoteDraft(source, draft);
    waiting.resolve({ notes: [savedNote({ id: "older" })], warnings: ["Notice"] });
    await pending;
    expect(source.notes.map((note) => note.id)).toEqual([draft.id, "older"]);
    expect(source.warnings).toEqual(["Notice"]);
  });

  it("retains cached notes and unsaved text on list errors rather than displaying silent empty results", async () => {
    const source = getReadingNotesSource(graphPath, "a", "A");
    source.notes = [savedNote()];
    const draft = newReadingNoteDraft(source);
    draft.body = "Keep draft";
    mocks.invoke.mockRejectedValue("Permission denied");
    await loadReadingNotes(source);
    expect(source.notes).toHaveLength(1);
    expect(source.listError).toContain("Permission denied");
    expect(draft.body).toBe("Keep draft");
    expect(source.loading).toBe(false);
  });
});
