/**
 * Changes to the block being edited that come from outside the editor, such as
 * the block actions in the right-click menu.
 *
 * While a block is edited its editor holds the text, and that can be ahead of
 * what is saved. A change made to the saved text would be overwritten when the
 * editor saves, so the change is made to the editor's own text instead and is
 * saved the way typing is.
 */
import type { EditorView } from "@codemirror/view";

function isHighSurrogate(code: number): boolean {
  return code >= 0xd800 && code <= 0xdbff;
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

/**
 * Makes the editor's text `next` by replacing only the part that differs, so
 * the caret and selection stay where they were and one undo step reverses it.
 * Returns false when there was nothing to change or the editor is gone.
 */
export function rewriteEditedBlock(view: EditorView, next: string): boolean {
  const current = view.state.doc.toString();
  if (next === current || !view.dom.isConnected) return false;

  const shortest = Math.min(current.length, next.length);
  let start = 0;
  while (start < shortest && current.charCodeAt(start) === next.charCodeAt(start)) start += 1;
  let currentEnd = current.length;
  let nextEnd = next.length;
  while (
    currentEnd > start &&
    nextEnd > start &&
    current.charCodeAt(currentEnd - 1) === next.charCodeAt(nextEnd - 1)
  ) {
    currentEnd -= 1;
    nextEnd -= 1;
  }

  // Never cut an emoji or other astral character in half.
  if (start > 0 && isHighSurrogate(current.charCodeAt(start - 1))) start -= 1;
  if (currentEnd < current.length && isLowSurrogate(current.charCodeAt(currentEnd))) {
    currentEnd += 1;
    nextEnd += 1;
  }

  view.dispatch({ changes: { from: start, to: currentEnd, insert: next.slice(start, nextEnd) } });
  view.focus();
  return true;
}
