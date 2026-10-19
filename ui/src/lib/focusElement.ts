import { EditorView } from "@codemirror/view";

/**
 * Gives focus back to an element once a menu or dialog closes.
 *
 * A CodeMirror editor is focused through its view, not its DOM node: the view
 * also puts the editor's own selection back into the page, which a bare
 * `focus()` on the content element does not.
 */
export function focusElement(element: Element | null | undefined): void {
  if (!(element instanceof HTMLElement) || !element.isConnected) return;
  const host = element.closest<HTMLElement>(".cm-editor");
  const view = host ? EditorView.findFromDOM(host) : null;
  if (view) {
    view.focus();
    return;
  }
  element.focus({ preventScroll: true });
}
