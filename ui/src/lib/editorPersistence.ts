import { Compartment, EditorState, Prec, Transaction, type Extension } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";

type FlushEditor = () => Promise<void>;

const editors = new Map<string, Set<FlushEditor>>();
const reloaders = new Map<string, Set<FlushEditor>>();
const locks = new Map<string, number>();
const lockListeners = new Map<string, Set<() => void>>();
const stalePages = new Set<string>();

export async function withPageEditorsLocked<T>(pageId: string, operation: () => Promise<T>): Promise<T> {
  locks.set(pageId, (locks.get(pageId) ?? 0) + 1);
  try {
    for (const update of lockListeners.get(pageId) ?? []) update();
    return await operation();
  } finally {
    const remaining = (locks.get(pageId) ?? 1) - 1;
    if (remaining) locks.set(pageId, remaining);
    else locks.delete(pageId);
    for (const update of lockListeners.get(pageId) ?? []) update();
  }
}

export function editorWriteLockExtension(pageId: string): Extension {
  const compartment = new Compartment();
  const configuration = () => locks.has(pageId)
    ? [EditorState.readOnly.of(true), EditorView.editable.of(false)] : [];
  return [
    Prec.highest(compartment.of(configuration())),
    EditorState.transactionFilter.of((transaction) =>
      locks.has(pageId) && transaction.docChanged && transaction.annotation(Transaction.addToHistory) !== false
        ? [] : transaction),
    ViewPlugin.define((view) => {
      const update = () => view.dispatch({ effects: compartment.reconfigure(configuration()) });
      let listeners = lockListeners.get(pageId);
      if (!listeners) lockListeners.set(pageId, listeners = new Set());
      listeners.add(update);
      // Cached editor states may have been created before the lock changed.
      queueMicrotask(() => { if (listeners.has(update)) update(); });
      return { destroy() {
        listeners.delete(update);
        if (!listeners.size && lockListeners.get(pageId) === listeners) lockListeners.delete(pageId);
      } };
    }),
  ];
}

export function registerEditorFlush(pageId: string, flush: FlushEditor): () => void {
  let pageEditors = editors.get(pageId);
  if (!pageEditors) editors.set(pageId, pageEditors = new Set());
  pageEditors.add(flush);
  return () => {
    pageEditors.delete(flush);
    if (!pageEditors.size && editors.get(pageId) === pageEditors) editors.delete(pageId);
  };
}

export async function flushPageEditors(pageId: string): Promise<void> {
  if (stalePages.has(pageId)) await reloadPageEditors(pageId);
  await Promise.all([...editors.get(pageId) ?? []].map((flush) => flush()));
}

export async function flushAllPageEditors(): Promise<void> {
  await Promise.all([...editors.keys()].map(flushPageEditors));
}

export function registerPageEditorReload(pageId: string, reload: FlushEditor): () => void {
  let pageReloaders = reloaders.get(pageId);
  if (!pageReloaders) reloaders.set(pageId, pageReloaders = new Set());
  pageReloaders.add(reload);
  return () => {
    pageReloaders.delete(reload);
    if (!pageReloaders.size && reloaders.get(pageId) === pageReloaders) reloaders.delete(pageId);
  };
}

export function markPageEditorsStale(pageId: string): void {
  if (stalePages.has(pageId)) return;
  stalePages.add(pageId);
  locks.set(pageId, (locks.get(pageId) ?? 0) + 1);
  for (const update of lockListeners.get(pageId) ?? []) update();
}

/** Called while editors are locked, after native source mutations and before allowing further typing. */
export async function reloadPageEditors(pageId: string): Promise<void> {
  try {
    await Promise.all([...reloaders.get(pageId) ?? []].map((reload) => reload()));
    if (stalePages.delete(pageId)) {
      const remaining = (locks.get(pageId) ?? 1) - 1;
      if (remaining) locks.set(pageId, remaining);
      else locks.delete(pageId);
      for (const update of lockListeners.get(pageId) ?? []) update();
    }
  } catch (error) {
    // A stale continuous document must never overwrite the newly written footnotes.
    markPageEditorsStale(pageId);
    throw error;
  }
}
