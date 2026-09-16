// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  readingNotesList, readingNoteCreate, readingNoteUpdate, readingNoteReattach, readingNoteSelection,
  readingNoteRelativePath, readingNotePassageTarget, renderReadingNoteBody, type ReadingNote,
} from "./readingNotes";
import { continuousReadingSelection, sourceReadingSelection } from "./readingSelection";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

const note: ReadingNote = {
  id: "note-1", notePageId: "note-page-1", filePath: "/synthetic/graph/pages/Reading notes/one.md",
  body: "A **useful** thought.", revision: "revision-1",
  source: { pageId: "source-1", pageTitle: "Book", filePath: "pages/Book.md" },
  quote: "Quoted words", status: "attached", statusMessage: "Exact passage found.", targetBlockId: "block-1",
  createdAt: "2026-09-15T00:00:00Z", updatedAt: "2026-09-15T00:00:00Z",
  storage: "file", footnoteLabel: null, noteBlockId: null,
};
const graph = "/synthetic/graph";

beforeEach(() => { vi.resetAllMocks(); mocks.invoke.mockResolvedValue(note); });

describe("reading note IPC", () => {
  it("omits pageId to list all notes, including missing sources", async () => {
    const result = { notes: [{ ...note, status: "orphaned", source: { ...note.source, pageId: null } }], warnings: ["Unreadable note file"] };
    mocks.invoke.mockResolvedValue(result);
    expect(await readingNotesList(graph)).toEqual(result);
    expect(mocks.invoke).toHaveBeenLastCalledWith("reading_notes_list", { graphPath: graph });
    await readingNotesList(graph, "source-1");
    expect(mocks.invoke).toHaveBeenLastCalledWith("reading_notes_list", { graphPath: graph, pageId: "source-1" });
  });

  it("preserves the full selection, caller UUID, revision and untrimmed Markdown body", async () => {
    const selection = sourceReadingSelection("source-1", "block-1", "Before quoted words after", 7, 19);
    await readingNoteCreate(graph, note.id, "source-1", selection, "  **Draft**\n");
    expect(mocks.invoke).toHaveBeenLastCalledWith("reading_note_create", {
      graphPath: graph, noteId: note.id, sourcePageId: "source-1", selection, body: "  **Draft**\n",
    });
    await readingNoteUpdate(graph, note.id, "revision-1", "Edited");
    expect(mocks.invoke).toHaveBeenLastCalledWith("reading_note_update", {
      graphPath: graph, noteId: note.id, expectedRevision: "revision-1", body: "Edited",
    });
    await readingNoteReattach(graph, note.id, "revision-2", "source-1", selection!);
    expect(mocks.invoke).toHaveBeenLastCalledWith("reading_note_reattach", {
      graphPath: graph, noteId: note.id, expectedRevision: "revision-2", sourcePageId: "source-1", selection,
    });
    expect(mocks.invoke.mock.lastCall?.[1]).not.toHaveProperty("body");
  });

  it("creates a page-level note with explicit null selection", async () => {
    await readingNoteCreate(graph, note.id, "source-1", null, "Whole-page thought");
    expect(mocks.invoke.mock.lastCall?.[1].selection).toBeNull();
  });

  it("does not swallow revision conflicts", async () => {
    const conflict = { code: "revision_conflict", message: "The file changed outside Grafium." };
    mocks.invoke.mockRejectedValue(conflict);
    await expect(readingNoteUpdate(graph, note.id, "old", "Draft")).rejects.toBe(conflict);
    await expect(readingNoteReattach(graph, note.id, "old", "source-1",
      sourceReadingSelection("source-1", "block", "quote", 0, 5)!)).rejects.toBe(conflict);
  });
});

describe("note source boundaries and previews", () => {
  it("copies classic UTF-16 ranges without widening them or retaining mutable parts", () => {
    const selection = sourceReadingSelection("day-a", "block-a", "Start 😀 words\nmore words End", 6, 26)!;
    const frozen = readingNoteSelection({ selection, error: null, pageIds: ["day-a"] }, "day-a")!;
    expect(frozen.parts[0]).toMatchObject({ from: 6, to: 26, prefix: "Start ", suffix: "End" });
    selection.parts[0].text = "Changed";
    selection.blockIds.push("wrong-block");
    expect(frozen.parts[0].text).toBe("😀 words\nmore words ");
    expect(frozen.blockIds).toEqual(["block-a"]);
    expect(readingNoteSelection({ selection, error: null, pageIds: ["day-a"] }, "day-b")).toBeNull();
  });

  it("preserves continuous source documentRange and multiline parts", () => {
    const source = "- Alpha line\n  second line\n  id:: a\n- Bravo\n  id:: b\n";
    const capture = continuousReadingSelection("book", source, source.indexOf("Alpha"), source.indexOf("Bravo") + 5);
    const selection = readingNoteSelection(capture, "book")!;
    expect(selection.parts.map((part) => part.text)).toEqual(["Alpha line\nsecond line", "Bravo"]);
    expect(selection.documentRange?.text).toContain("id:: a");
    capture.selection!.documentRange!.text = "Changed";
    expect(selection.documentRange?.text).not.toBe("Changed");
  });

  it("rejects cross-page captures instead of silently creating a page-level note", () => {
    expect(() => readingNoteSelection({
      selection: null, pageIds: ["day-a", "day-b"], error: "Select within one journal day.",
    }, "day-a")).toThrow("one journal day");
  });

  it("navigates only by verified source/block IDs, never by title guesses", () => {
    expect(readingNotePassageTarget(note)).toEqual({ pageId: "source-1", pageName: "Book", targetBlockId: "block-1" });
    expect(readingNotePassageTarget({ ...note, status: "recovered" })).not.toBeNull();
    for (const status of ["ambiguous", "orphaned"] as const) expect(readingNotePassageTarget({ ...note, status })).toBeNull();
    expect(readingNotePassageTarget({ ...note, targetBlockId: null })).toBeNull();
    expect(readingNotePassageTarget({ ...note, source: { ...note.source, pageId: null } })).toBeNull();
  });

  it("resolves absolute and relative note paths against the actual graph root", () => {
    expect(readingNoteRelativePath(graph, note.filePath)).toBe("pages/Reading notes/one.md");
    expect(readingNoteRelativePath(graph, "pages/Reading notes/one.md")).toBe("pages/Reading notes/one.md");
    expect(readingNoteRelativePath("C:\\graphs\\one", "C:\\graphs\\one\\pages\\note.md")).toBe("pages/note.md");
    const html = renderReadingNoteBody({ ...note, body: "![Figure](assets/figure.png)" }, graph);
    expect(html).toContain("grafium-asset://localhost/pages/Reading%20notes/assets/figure.png");
    expect(html).not.toContain("/synthetic/graph");
  });

  it("renders Markdown without executable HTML or actionable preview page links", () => {
    const html = renderReadingNoteBody({
      ...note, body: '**Bold**\n\n[[Never create this]]\n\n<script>alert(1)</script><img src="x" onerror="alert(1)">\n\n[bad](javascript:alert)',
    }, graph);
    const preview = document.createElement("div");
    preview.innerHTML = html;
    expect(preview.querySelector("strong")?.textContent).toBe("Bold");
    expect(preview.querySelector("script, [onerror], [href], [data-page], [data-page-link], [data-ref]")).toBeNull();
    expect(preview.textContent).toContain("Never create this");
  });
});
