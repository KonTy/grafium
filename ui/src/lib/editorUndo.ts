import { isolateHistory, redo, redoDepth, undo, undoDepth } from "@codemirror/commands";
import { Transaction, type ChangeSpec, type EditorState, type Text } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { isUndoOperationInProgress, peekRedoAction, peekUndoAction, type DeleteBlockSelectionAction, type WritingRewriteAction } from "./undoStack";

export const EDITOR_UNDO_MIN_DEPTH = 50;

interface StructuralUndoBoundary {
  action: DeleteBlockSelectionAction | WritingRewriteAction;
  doc: Text;
  undoDepth: number;
  redoBoundary?: { undoDepth: number; redoDepth: number };
  pending?: "undo" | "redo";
}

const boundaries = new WeakMap<EditorView, StructuralUndoBoundary>();
const recordedBoundaries = new WeakMap<EditorView, WeakMap<StructuralUndoBoundary["action"], StructuralUndoBoundary>>();
const writingStates = new WeakMap<EditorView, WeakMap<WritingRewriteAction, { before: EditorState; after: EditorState }>>();

function boundaryFor(view: EditorView, action: ReturnType<typeof peekUndoAction>) {
  return (action?.type === "rewrite_writing" || action?.type === "delete_block_selection"
    ? recordedBoundaries.get(view)?.get(action) : undefined) ?? boundaries.get(view);
}

/** Restore native history only when the entire document still matches its saved boundary. */
export function applyWritingEditorChanges(
  view: EditorView, changes: ChangeSpec, action: WritingRewriteAction, undoing: boolean,
): void {
  view.dispatch({ annotations: [isolateHistory.of("full"), Transaction.addToHistory.of(false)] });
  let states = writingStates.get(view);
  if (!states) writingStates.set(view, states = new WeakMap());
  const saved = states.get(action);
  if (saved && view.state.doc.eq(undoing ? saved.after.doc : saved.before.doc)) {
    const restored = undoing ? saved.before : saved.after;
    if (undoing) saved.after = view.state;
    else saved.before = view.state;
    view.setState(restored);
  } else {
    const before = view.state;
    view.dispatch({ changes, annotations: Transaction.addToHistory.of(false) });
    if (!undoing) states.set(action, { before, after: view.state });
    else states.delete(action);
  }
  if (undoing) {
    const boundary = recordedBoundaries.get(view)?.get(action);
    if (boundary) {
      boundary.pending = undefined;
      boundary.redoBoundary = { undoDepth: undoDepth(view.state), redoDepth: redoDepth(view.state) };
    }
  } else {
    markStructuralUndoBoundary(view);
  }
}

/** Call after focusing the surviving editor following a grouped deletion. */
export function markStructuralUndoBoundary(view: EditorView): void {
  const action = peekUndoAction();
  if (action?.type !== "delete_block_selection" && action?.type !== "rewrite_writing") {
    boundaries.delete(view);
    recordedBoundaries.delete(view);
    return;
  }
  // A recently cached edit must not merge with typing after structural focus.
  view.dispatch({ annotations: [isolateHistory.of("full"), Transaction.addToHistory.of(false)] });
  const boundary: StructuralUndoBoundary = {
    action,
    doc: view.state.doc,
    undoDepth: undoDepth(view.state),
  };
  boundaries.set(view, boundary);
  let recorded = recordedBoundaries.get(view);
  if (!recorded) recordedBoundaries.set(view, recorded = new WeakMap());
  recorded.set(action, boundary);
}

function requestStructuralHistory(boundary: StructuralUndoBoundary, direction: "undo" | "redo"): boolean {
  boundary.pending = direction;
  window.dispatchEvent(new CustomEvent(`app-${direction}`));
  return true;
}

function structuralRequestInProgress(boundary: StructuralUndoBoundary): boolean {
  if (!isUndoOperationInProgress()) boundary.pending = undefined;
  return !!boundary.pending;
}

/** CodeMirror-compatible command; callers keep their existing empty-history fallback. */
export function undoEditor(view: EditorView): boolean {
  const boundary = boundaryFor(view, peekUndoAction());
  if (boundary) {
    if (structuralRequestInProgress(boundary)) {
      return requestStructuralHistory(boundary, "undo");
    }
    if (peekUndoAction() === boundary.action
      && undoDepth(view.state) === boundary.undoDepth
      && view.state.doc.eq(boundary.doc)) {
      boundary.redoBoundary = { undoDepth: undoDepth(view.state), redoDepth: redoDepth(view.state) };
      return requestStructuralHistory(boundary, "undo");
    }
  }
  return undo(view);
}

/** Redo the structural action before typing that was undone back to its boundary. */
export function redoEditor(view: EditorView): boolean {
  const boundary = boundaryFor(view, peekRedoAction());
  if (boundary) {
    if (structuralRequestInProgress(boundary)) {
      return requestStructuralHistory(boundary, "redo");
    }
    const restored = boundary.redoBoundary;
    // An in-place, non-history reload can change the document while preserving
    // these depths. A replacement view has no marker and uses the caller's
    // existing empty-history app fallback instead.
    if (peekRedoAction() === boundary.action && restored
      && undoDepth(view.state) === restored.undoDepth
      && redoDepth(view.state) === restored.redoDepth) {
      return requestStructuralHistory(boundary, "redo");
    }
  }
  return redo(view);
}
