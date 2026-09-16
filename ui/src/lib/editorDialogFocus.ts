const retainedEditors = new WeakSet<HTMLElement>();

/** Keep the invoking editor intact while a temporary dialog borrows focus. */
export function retainEditorForDialog(editor: HTMLElement): () => void {
  retainedEditors.add(editor);
  return () => { retainedEditors.delete(editor); };
}

export function editorHasDialogFocus(editor: HTMLElement): boolean {
  return retainedEditors.has(editor);
}
