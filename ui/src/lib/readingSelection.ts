import { writable } from "svelte/store";
import { parsePageSourceMap } from "./pageSourceMap";
import { renderedReadingText } from "./renderedReadingText";

export interface ReadingSelectionPart {
  blockId: string;
  text: string;
  /** UTF-16 offsets in source text, or renderedReadingText for kind="rendered". */
  from: number;
  to: number;
  prefix: string;
  suffix: string;
}

export interface ReadingSelection {
  pageId: string;
  blockIds: string[];
  text: string;
  kind: "source" | "rendered";
  parts: ReadingSelectionPart[];
  /** Original document offsets/text, including structural Markdown in continuous mode. */
  documentRange?: { from: number; to: number; text: string };
}

export interface ReadingSelectionCapture {
  selection: ReadingSelection | null;
  error: string | null;
  pageIds: string[];
}

export const readingSelection = writable<ReadingSelectionCapture>({ selection: null, error: null, pageIds: [] });

function part(blockId: string, text: string, from: number, to: number): ReadingSelectionPart {
  // Context windows must not send half a UTF-16 surrogate pair to native JSON.
  const prefix = text.slice(Math.max(0, from - 64), from).replace(/^[\uDC00-\uDFFF]/, "");
  const suffix = text.slice(to, to + 64).replace(/[\uD800-\uDBFF]$/, "");
  return { blockId, text: text.slice(from, to), from, to, prefix, suffix };
}

export function sourceReadingSelection(pageId: string, blockId: string, text: string, from: number, to: number): ReadingSelection | null {
  if (!pageId || !blockId || from < 0 || to > text.length || to <= from || !text.slice(from, to).trim()) return null;
  return { pageId, blockIds: [blockId], text: text.slice(from, to), kind: "source", parts: [part(blockId, text, from, to)] };
}

export function continuousReadingSelection(pageId: string, source: string, from: number, to: number): ReadingSelectionCapture {
  const parts: ReadingSelectionPart[] = [];
  for (const block of parsePageSourceMap(source).blocks) {
    if (block.readingNote?.storage === "inline") {
      if (from < block.ownTo && to > block.ownFrom) {
        return { selection: null, error: "Select book text, not an annotation footer.", pageIds: [pageId] };
      }
      continue;
    }
    const excerpts: ReadingSelectionPart[] = [];
    for (const segment of block.contentSegments) {
      const start = Math.max(from, segment.from);
      const end = Math.min(to, segment.to);
      if (start < end) {
        if (!block.id) return { selection: null, error: "This passage has no saved block ID. Save any edits, then use the classic editor for passage selection.", pageIds: [pageId] };
        excerpts.push(part(block.id, block.content, segment.contentFrom + start - segment.from, segment.contentFrom + end - segment.from));
      }
    }
    if (excerpts.length) {
      parts.push(part(block.id!, block.content, excerpts[0].from, excerpts[excerpts.length - 1].to));
    }
  }
  const text = parts.map((p) => p.text).join("\n");
  return {
    selection: text.trim() ? {
      pageId, blockIds: parts.map((p) => p.blockId), text, kind: "source", parts,
      documentRange: { from, to, text: source.slice(from, to) },
    } : null,
    error: null, pageIds: [pageId],
  };
}

function elementFor(node: Node | null): Element | null {
  return node instanceof Element ? node : node?.parentElement ?? null;
}

function readingEndpoint(node: Node | null) {
  const element = elementFor(node);
  if (!element?.closest(".main-content .page-content")) return null;
  if (element.closest("[data-reading-note-footer]")) return null;
  const content = element.closest<HTMLElement>(".rendered-content, .cm-content");
  if (!content) return null;
  const block = content.closest<HTMLElement>("[data-block-id], [data-source-block-id]");
  const page = content.closest<HTMLElement>("[data-page-id]");
  return page?.dataset.pageId ? { content, block, pageId: page.dataset.pageId } : null;
}

/** Only left-hand note content is eligible; never harvest a transcript or another panel. */
export function captureRenderedReadingSelection(selection: Selection | null): ReadingSelectionCapture | null {
  if (!selection || selection.isCollapsed || !selection.rangeCount || !selection.toString().trim()) return null;
  const start = readingEndpoint(selection.anchorNode);
  const end = readingEndpoint(selection.focusNode);
  if (!start && !end) return null;
  const pageIds = [...new Set([start?.pageId, end?.pageId].filter((id): id is string => !!id))];
  if (!start || !end || start.pageId !== end.pageId || selection.rangeCount !== 1) {
    return { selection: null, error: "Select text within one page or journal day, not across pages or panels.", pageIds };
  }
  // CodeMirror's document selection is authoritative, not its virtualized DOM.
  if (start.content.classList.contains("cm-content") || end.content.classList.contains("cm-content")) return null;
  const range = selection.getRangeAt(0);
  const parts: ReadingSelectionPart[] = [];
  for (const content of document.querySelectorAll<HTMLElement>(".main-content .page-content .rendered-content")) {
    if (!range.intersectsNode(content)) continue;
    const endpoint = readingEndpoint(content);
    if (!endpoint?.block || endpoint.pageId !== start.pageId) {
      return { selection: null, error: "Select text within one saved page or journal day.", pageIds };
    }
    const selected = document.createRange();
    selected.selectNodeContents(content);
    if (content.contains(range.startContainer)) selected.setStart(range.startContainer, range.startOffset);
    if (content.contains(range.endContainer)) selected.setEnd(range.endContainer, range.endOffset);
    const representation = renderedReadingText(content);
    const from = representation.offsetAt(selected.startContainer, selected.startOffset);
    const to = representation.offsetAt(selected.endContainer, selected.endOffset);
    const blockId = endpoint.block.dataset.blockId ?? endpoint.block.dataset.sourceBlockId;
    if (blockId && to > from) parts.push(part(blockId, representation.text, from, to));
  }
  const text = parts.map((p) => p.text).join("\n");
  return { selection: text.trim() ? { pageId: start.pageId, blockIds: parts.map((p) => p.blockId), text, kind: "rendered", parts } : null, error: null, pageIds };
}

export function publishSourceReadingSelection(root: Element, selection: ReadingSelection | null): void {
  if (selection && root.closest(".main-content .page-content") && !root.closest("[data-reading-note-footer]")) {
    readingSelection.set({ selection, error: null, pageIds: [selection.pageId] });
  }
}

export function publishContinuousReadingSelection(root: Element, pageId: string, source: string, from: number, to: number): void {
  if (from === to || !root.closest(".main-content .page-content")) return;
  const captured = continuousReadingSelection(pageId, source, from, to);
  if (captured.selection || captured.error) readingSelection.set(captured);
}

let readers = 0;
function capture() {
  const selectedPages = new Set<string>();
  for (const block of document.querySelectorAll<HTMLElement>(".main-content .page-content .block-item.selected, .main-content .page-content .unified-rendered-block.selected")) {
    const pageId = block.closest<HTMLElement>("[data-page-id]")?.dataset.pageId;
    if (pageId) selectedPages.add(pageId);
  }
  if (selectedPages.size > 1) {
    readingSelection.set({ selection: null, error: "Select text within one page or journal day, not across pages.", pageIds: [...selectedPages] });
    return;
  }
  const captured = captureRenderedReadingSelection(window.getSelection());
  if (captured) readingSelection.set(captured);
}

/** Installed on either editor shell, so selection is frozen even while the panel is closed. */
export function readingSelectionCapture(_node: HTMLElement) {
  if (readers++ === 0) {
    document.addEventListener("selectionchange", capture);
    document.addEventListener("pointerdown", capture, true);
    document.addEventListener("mouseup", capture);
  }
  return { destroy() {
    if (--readers === 0) {
      document.removeEventListener("selectionchange", capture);
      document.removeEventListener("pointerdown", capture, true);
      document.removeEventListener("mouseup", capture);
    }
  } };
}
