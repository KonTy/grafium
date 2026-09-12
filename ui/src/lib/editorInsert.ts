import { EditorSelection } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

export const EDIT_PAGE_END_EVENT = "grafium-edit-page-end";
export const PERSONAL_JOURNAL_LINK = "[[personal/journal]]";

export type EditPageEndDetail = {
  pageTitle?: string;
  pageId?: string;
  insert?: string;
};

/** Local 24h clock, e.g. `14:07`. */
export function formatLocalClockTime(date = new Date()): string {
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/** Timestamp plus a newline so the caret lands on the next line. */
export function timeStampSnippet(date = new Date()): string {
  return `${formatLocalClockTime(date)}\n`;
}

export function personalJournalSnippet(): string {
  return `${PERSONAL_JOURNAL_LINK}\n`;
}

export function insertAtCursor(view: EditorView, text: string): void {
  const { from, to } = view.state.selection.main;
  view.dispatch({
    changes: { from, to, insert: text },
    selection: EditorSelection.cursor(from + text.length),
  });
  view.focus();
}

export function activeEditorView(): EditorView | undefined {
  return (window as unknown as { __activeEditorView?: EditorView }).__activeEditorView;
}

export function tryInsertIntoActiveEditor(text: string): boolean {
  const view = activeEditorView();
  if (!view) return false;
  insertAtCursor(view, text);
  return true;
}

let pendingEditPageEnd: EditPageEndDetail | null = null;

export function dispatchEditPageEnd(detail: EditPageEndDetail): void {
  pendingEditPageEnd = { ...detail };
  window.dispatchEvent(new CustomEvent(EDIT_PAGE_END_EVENT, { detail: pendingEditPageEnd }));
}

export function peekPendingEditPageEnd(page: { id: string; title: string }): EditPageEndDetail | null {
  const pending = pendingEditPageEnd;
  if (!pending) return null;
  if (pending.pageId && pending.pageId !== page.id) return null;
  if (pending.pageTitle && pending.pageTitle !== page.title) return null;
  if (!pending.pageId && !pending.pageTitle) return null;
  return pending;
}

export function clearPendingEditPageEnd(): void {
  pendingEditPageEnd = null;
}

export function waitForActiveEditor(timeoutMs = 1500): Promise<EditorView | null> {
  const started = Date.now();
  return new Promise((resolve) => {
    const poll = () => {
      const view = activeEditorView();
      if (view) {
        resolve(view);
        return;
      }
      if (Date.now() - started > timeoutMs) {
        resolve(null);
        return;
      }
      requestAnimationFrame(poll);
    };
    poll();
  });
}
