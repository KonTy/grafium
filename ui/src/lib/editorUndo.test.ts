import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { history, redoDepth, undoDepth } from "@codemirror/commands";
import { EditorState, Transaction } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { applyWritingEditorChanges, redoEditor as editorRedo, undoEditor as editorUndo, markStructuralUndoBoundary } from "./editorUndo";
import { isUndoOperationInProgress, peekRedoAction, peekUndoAction, type DeleteBlockSelectionAction, type UndoAction, type WritingRewriteAction } from "./undoStack";

vi.mock("./undoStack", () => ({
  peekUndoAction: vi.fn(),
  peekRedoAction: vi.fn(),
  isUndoOperationInProgress: vi.fn(),
}));

const views: EditorView[] = [];
let topUndo: UndoAction | undefined;
let topRedo: UndoAction | undefined;
let action: DeleteBlockSelectionAction;
const appUndo = vi.fn();
const appRedo = vi.fn();

function editor(doc = "old"): EditorView {
  const view = new EditorView({
    state: EditorState.create({ doc, extensions: [history()] }),
    parent: document.body,
  });
  views.push(view);
  return view;
}

function type(view: EditorView, text: string) {
  view.dispatch({
    changes: { from: view.state.doc.length, insert: text },
    annotations: Transaction.userEvent.of("input.type"),
  });
}

function completeUndo() {
  topUndo = undefined;
  topRedo = action;
}

function completeRedo() {
  topRedo = undefined;
  topUndo = action;
}

beforeEach(() => {
  vi.resetAllMocks();
  action = { type: "delete_block_selection", pageId: "day", groups: [], placeholderIds: {} };
  topUndo = action;
  topRedo = undefined;
  vi.mocked(peekUndoAction).mockImplementation(() => topUndo);
  vi.mocked(peekRedoAction).mockImplementation(() => topRedo);
  vi.mocked(isUndoOperationInProgress).mockReturnValue(false);
  window.addEventListener("app-undo", appUndo);
  window.addEventListener("app-redo", appRedo);
});

afterEach(() => {
  window.removeEventListener("app-undo", appUndo);
  window.removeEventListener("app-redo", appRedo);
  for (const view of views.splice(0)) view.destroy();
});

describe("structural editor undo boundary", () => {
  it("restores older native history and newer redo across an arbitrary full rewrite", () => {
    const view = editor("Original");
    type(view, " earlier");
    const rewrite: WritingRewriteAction = {
      type: "rewrite_writing", graphPath: "/tmp/graph", pageId: "day",
      changes: [{ blockId: "one", beforeContent: "Original earlier", afterContent: "Entirely different prose" }],
    };
    const apply = (content: string, undoing: boolean) =>
      applyWritingEditorChanges(view, { from: 0, to: view.state.doc.length, insert: content }, rewrite, undoing);
    topUndo = rewrite;
    apply("Entirely different prose", false);
    type(view, " later");
    expect(editorUndo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("Entirely different prose");
    expect(editorUndo(view)).toBe(true);
    expect(appUndo).toHaveBeenCalledOnce();
    topUndo = undefined;
    topRedo = rewrite;
    apply("Original earlier", true);
    expect(editorUndo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("Original");
    expect(editorRedo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("Original earlier");
    expect(editorRedo(view)).toBe(true);
    expect(appRedo).toHaveBeenCalledOnce();
    topUndo = rewrite;
    topRedo = undefined;
    apply("Entirely different prose", false);
    expect(editorRedo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("Entirely different prose later");
  });

  it("retains the preceding structural boundary across two successive rewrites", () => {
    const view = editor("Original");
    type(view, " earlier");
    const first: WritingRewriteAction = {
      type: "rewrite_writing", graphPath: "/tmp/graph", pageId: "day",
      changes: [{ blockId: "one", beforeContent: "Original earlier", afterContent: "First rewrite" }],
    };
    const second: WritingRewriteAction = {
      ...first, changes: [{ blockId: "one", beforeContent: "First rewrite", afterContent: "Second rewrite" }],
    };
    topUndo = first;
    applyWritingEditorChanges(view, { from: 0, to: view.state.doc.length, insert: "First rewrite" }, first, false);
    topUndo = second;
    applyWritingEditorChanges(view, { from: 0, to: view.state.doc.length, insert: "Second rewrite" }, second, false);
    topUndo = first;
    topRedo = second;
    applyWritingEditorChanges(view, { from: 0, to: view.state.doc.length, insert: "First rewrite" }, second, true);
    expect(editorUndo(view)).toBe(true);
    expect(appUndo).toHaveBeenCalledOnce();
    expect(view.state.doc.toString()).toBe("First rewrite");
  });

  it("never restores an old whole-page snapshot over unrelated changes", () => {
    const view = editor("Old\nKeep");
    const rewrite: WritingRewriteAction = {
      type: "rewrite_writing", graphPath: "/tmp/graph", pageId: "day",
      changes: [{ blockId: "one", beforeContent: "Old", afterContent: "New" }],
    };
    topUndo = rewrite;
    applyWritingEditorChanges(view, { from: 0, to: 3, insert: "New" }, rewrite, false);
    view.dispatch({ changes: { from: view.state.doc.length, insert: " external" }, annotations: Transaction.addToHistory.of(false) });
    topUndo = undefined;
    topRedo = rewrite;
    applyWritingEditorChanges(view, { from: 0, to: 3, insert: "Old" }, rewrite, true);
    expect(view.state.doc.toString()).toBe("Old\nKeep external");
  });

  it("prioritizes the whole natural rewrite before old edits and after new typing", () => {
    const view = editor("Original");
    type(view, " cached");
    topUndo = {
      type: "rewrite_writing", graphPath: "/tmp/graph", pageId: "day",
      changes: [{ blockId: "one", beforeContent: "Original cached", afterContent: "Natural" }],
    };
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: "Natural" },
      annotations: Transaction.addToHistory.of(false),
    });
    markStructuralUndoBoundary(view);
    type(view, " later");
    expect(editorUndo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("Natural");
    expect(appUndo).not.toHaveBeenCalled();
    expect(editorUndo(view)).toBe(true);
    expect(appUndo).toHaveBeenCalledOnce();
    expect(view.state.doc.toString()).toBe("Natural");
  });

  it("prioritizes grouped deletion over cached survivor edits without clearing them", () => {
    const view = editor();
    type(view, " cached");
    markStructuralUndoBoundary(view);
    expect(undoDepth(view.state)).toBe(1);
    expect(editorUndo(view)).toBe(true);
    expect(appUndo).toHaveBeenCalledTimes(1);
    expect(view.state.doc.toString()).toBe("old cached");
    expect(undoDepth(view.state)).toBe(1);
    completeUndo();
    expect(editorUndo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("old");
    expect(appUndo).toHaveBeenCalledTimes(1);
  });

  it("undoes new typing locally to the boundary, then the group, and redoes the group before typing", () => {
    const view = editor();
    type(view, " cached");
    markStructuralUndoBoundary(view);
    type(view, " new");
    expect(undoDepth(view.state)).toBe(2);
    expect(editorUndo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("old cached");
    expect(appUndo).not.toHaveBeenCalled();
    expect(editorUndo(view)).toBe(true);
    expect(appUndo).toHaveBeenCalledTimes(1);
    completeUndo();
    expect(editorRedo(view)).toBe(true);
    expect(appRedo).toHaveBeenCalledTimes(1);
    expect(view.state.doc.toString()).toBe("old cached");
    completeRedo();
    expect(editorRedo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("old cached new");
    expect(appRedo).toHaveBeenCalledTimes(1);
  });

  it("does not prioritize a different structural action or other app action", () => {
    const view = editor();
    type(view, " cached");
    markStructuralUndoBoundary(view);
    topUndo = { ...action };
    expect(editorUndo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("old");
    expect(appUndo).not.toHaveBeenCalled();
    topUndo = { type: "update_block", pageId: "day", blockId: "other", beforeContent: "", afterContent: "other" };
    expect(editorRedo(view)).toBe(true);
    expect(appRedo).not.toHaveBeenCalled();
  });

  it("does not intercept a different editor using identical cached content", () => {
    const view = editor();
    const other = editor();
    type(view, " cached");
    type(other, " cached");
    markStructuralUndoBoundary(view);
    expect(editorUndo(other)).toBe(true);
    expect(other.state.doc.toString()).toBe("old");
    expect(appUndo).not.toHaveBeenCalled();
  });

  it("does not intercept changed documents with the same undo depth", () => {
    const view = editor();
    type(view, " cached");
    markStructuralUndoBoundary(view);
    view.dispatch({
      changes: { from: 0, insert: "external " },
      annotations: Transaction.addToHistory.of(false),
    });
    expect(editorUndo(view)).toBe(true);
    expect(appUndo).not.toHaveBeenCalled();
  });

  it("does not prioritize group redo after new typing or an unrelated redo action", () => {
    const view = editor();
    type(view, " cached");
    markStructuralUndoBoundary(view);
    editorUndo(view);
    completeUndo();
    type(view, " after restore");
    expect(editorRedo(view)).toBe(false);
    expect(appRedo).not.toHaveBeenCalled();
    editorUndo(view);
    topRedo = { ...action };
    expect(editorRedo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("old cached after restore");
    expect(appRedo).not.toHaveBeenCalled();
  });

  it("prioritizes group redo after an in-place non-history block reload", () => {
    const view = editor("survivor");
    type(view, " cached");
    markStructuralUndoBoundary(view);
    type(view, " new");
    editorUndo(view);
    editorUndo(view);
    completeUndo();
    const depths = [undoDepth(view.state), redoDepth(view.state)];
    view.dispatch({
      changes: { from: 0, insert: "restored block\n" },
      annotations: Transaction.addToHistory.of(false),
    });
    expect([undoDepth(view.state), redoDepth(view.state)]).toEqual(depths);
    expect(editorRedo(view)).toBe(true);
    expect(appRedo).toHaveBeenCalledTimes(1);
    expect(view.state.doc.toString()).toBe("restored block\nsurvivor cached");
    completeRedo();
    view.dispatch({
      changes: { from: 0, to: "restored block\n".length },
      annotations: Transaction.addToHistory.of(false),
    });
    expect(editorRedo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("survivor cached new");
  });

  it("leaves a replacement view's empty-history commands to the existing app fallback", () => {
    const view = editor("survivor");
    markStructuralUndoBoundary(view);
    expect(editorUndo(view)).toBe(true);
    completeUndo();
    const replacement = editor("restored block\nsurvivor");
    expect(editorRedo(replacement)).toBe(false);
    expect(appRedo).not.toHaveBeenCalled();
    expect(editorUndo(replacement)).toBe(false);
    expect(appUndo).toHaveBeenCalledTimes(1);
  });

  it("retries failed grouped undo and redo instead of consuming cached text history", () => {
    const view = editor();
    type(view, " cached");
    markStructuralUndoBoundary(view);
    editorUndo(view);
    editorUndo(view);
    expect(appUndo).toHaveBeenCalledTimes(2);
    completeUndo();
    editorRedo(view);
    editorRedo(view);
    expect(appRedo).toHaveBeenCalledTimes(2);
    expect(view.state.doc.toString()).toBe("old cached");
  });

  it("does not undo cached text while a requested structural operation is still running", () => {
    const view = editor();
    type(view, " cached");
    markStructuralUndoBoundary(view);
    editorUndo(view);
    topUndo = undefined;
    vi.mocked(isUndoOperationInProgress).mockReturnValue(true);
    expect(editorUndo(view)).toBe(true);
    expect(editorRedo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("old cached");
  });

  it("clears a previous marker when marking without a matching grouped action", () => {
    const view = editor();
    type(view, " cached");
    markStructuralUndoBoundary(view);
    topUndo = undefined;
    markStructuralUndoBoundary(view);
    topUndo = action;
    expect(editorUndo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("old");
    expect(appUndo).not.toHaveBeenCalled();
  });
});
