import { invoke } from "@tauri-apps/api/core";
import DOMPurify from "dompurify";
import { assetBaseDirFor, renderBlock } from "./markdown";
import { validReadingNoteLabel } from "./readingNoteFormat";
import type { ReadingSelection, ReadingSelectionCapture } from "./readingSelection";

export interface ReadingNote {
  id: string;
  notePageId: string;
  filePath: string;
  body: string;
  revision: string;
  source: { pageId: string | null; pageTitle: string; filePath: string };
  quote: string;
  status: "attached" | "recovered" | "ambiguous" | "orphaned";
  statusMessage: string;
  targetBlockId: string | null;
  createdAt: string;
  updatedAt: string;
  storage: "inline" | "file";
  footnoteLabel: string | null;
  noteBlockId: string | null;
}

export interface ReadingNotesList {
  notes: ReadingNote[];
  warnings: string[];
}

export function readingNotesList(graphPath: string, pageId?: string): Promise<ReadingNotesList> {
  return invoke("reading_notes_list", { graphPath, ...(pageId ? { pageId } : {}) });
}

export function readingNoteCreate(
  graphPath: string, noteId: string, sourcePageId: string, selection: ReadingSelection | null, body: string,
): Promise<ReadingNote> {
  return invoke("reading_note_create", { graphPath, noteId, sourcePageId, selection, body });
}

export function readingNoteUpdate(
  graphPath: string, noteId: string, expectedRevision: string, body: string,
): Promise<ReadingNote> {
  return invoke("reading_note_update", { graphPath, noteId, expectedRevision, body });
}

export function readingNoteReattach(
  graphPath: string, noteId: string, expectedRevision: string, sourcePageId: string, selection: ReadingSelection,
): Promise<ReadingNote> {
  return invoke("reading_note_reattach", { graphPath, noteId, expectedRevision, sourcePageId, selection });
}

export function cloneReadingSelection(selection: ReadingSelection | null): ReadingSelection | null {
  if (!selection) return null;
  return {
    ...selection,
    blockIds: [...selection.blockIds],
    parts: selection.parts.map((part) => ({ ...part })),
    ...(selection.documentRange ? { documentRange: { ...selection.documentRange } } : {}),
  };
}

export function readingNoteSelection(captured: ReadingSelectionCapture, pageId: string): ReadingSelection | null {
  if (captured.pageIds.includes(pageId) && captured.error) throw new Error(captured.error);
  const selection = captured.selection;
  if (!selection || selection.pageId !== pageId) return null;
  if (!selection.text.trim() || !selection.parts.length || !selection.blockIds.length) {
    throw new Error("Select text in saved blocks on this page first.");
  }
  return cloneReadingSelection(selection);
}

/** DTO paths may be absolute; Markdown's renderer expects a graph-relative path. */
export function readingNoteRelativePath(graphPath: string, filePath: string): string {
  const graph = graphPath.replace(/\\/g, "/").replace(/\/+$/, "");
  const file = filePath.replace(/\\/g, "/");
  return file.startsWith(`${graph}/`) ? file.slice(graph.length + 1) : file;
}

export function readingNotePassageTarget(note: ReadingNote) {
  if (!["attached", "recovered"].includes(note.status) || !note.source.pageId || !note.targetBlockId) return null;
  return { pageId: note.source.pageId, pageName: note.source.pageTitle, targetBlockId: note.targetBlockId };
}

export function readingNoteLocationTarget(note: ReadingNote) {
  return note.noteBlockId
    ? { pageId: note.notePageId, pageName: note.source.pageTitle, targetBlockId: note.noteBlockId }
    : null;
}

export function renderReadingNoteBody(note: ReadingNote, graphPath: string): string {
  return renderReadingNoteMarkdown(note.body, assetBaseDirFor(readingNoteRelativePath(graphPath, note.filePath)));
}

export function renderReadingNoteMarkdown(body: string, baseDir: string): string {
  const html = renderBlock(body, baseDir);
  return DOMPurify.sanitize(html, {
    ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|grafium-asset:|#|\/)/i,
    FORBID_TAGS: ["script", "style", "iframe", "object", "embed", "form", "input", "button"],
    // This is a read-only preview, not a page-link or task editor.
    FORBID_ATTR: ["style", "href", "data-page", "data-page-link", "data-tag", "data-ref", "data-block-ref", "data-reading-note-label", "autoplay"],
  });
}

export function renderReadingNoteFooter(body: string, label: string, baseDir: string): string {
  if (!validReadingNoteLabel(label)) return renderReadingNoteMarkdown(body, baseDir);
  const number = label.slice("grafium-note-".length);
  return `<section class="reading-note-footer" data-reading-note-footer="" aria-label="Reading note ${number}"><a class="reading-note-footer-label" href="#${label}" data-reading-note-label="${label}" aria-label="Open reading note ${number}">Note ${number}</a><div class="reading-note-footer-body">${renderReadingNoteMarkdown(body, baseDir)}</div></section>`;
}
