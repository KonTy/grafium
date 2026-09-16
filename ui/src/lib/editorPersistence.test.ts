import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState, Transaction } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { editorWriteLockExtension, flushAllPageEditors, flushPageEditors, registerEditorFlush, withPageEditorsLocked, registerPageEditorReload, reloadPageEditors } from "./editorPersistence";

const views: EditorView[] = [];
const unregister: (() => void)[] = [];
function editor(pageId: string, state?: EditorState) {
  const view = new EditorView({
    parent: document.body,
    state: state ?? EditorState.create({ doc: "Original", extensions: [editorWriteLockExtension(pageId)] }),
  });
  views.push(view);
  return view;
}
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  for (const dispose of unregister.splice(0)) dispose();
});

describe("page editor persistence", () => {
  it("flushes each mounted page for graph/book questions and propagates failures", async () => {
    const first = vi.fn().mockResolvedValue(undefined);
    const second = vi.fn().mockRejectedValue(new Error("Could not save chapter"));
    unregister.push(registerEditorFlush("one", first), registerEditorFlush("two", second));
    await expect(flushAllPageEditors()).rejects.toThrow("Could not save chapter");
    expect(first).toHaveBeenCalledOnce();
    expect(second).toHaveBeenCalledOnce();
  });

  it("flushes only registered editors on the requested page and propagates save errors", async () => {
    const first = vi.fn().mockResolvedValue(undefined);
    const second = vi.fn().mockRejectedValue(new Error("Save failed"));
    const other = vi.fn().mockResolvedValue(undefined);
    unregister.push(registerEditorFlush("one", first), registerEditorFlush("one", second), registerEditorFlush("two", other));
    await expect(flushPageEditors("one")).rejects.toThrow("Save failed");
    expect(first).toHaveBeenCalledOnce();
    expect(second).toHaveBeenCalledOnce();
    expect(other).not.toHaveBeenCalled();
  });

  it("locks only the target page, permits persisted replacements, and restores editing", async () => {
    const view = editor("one");
    const other = editor("two");
    await withPageEditorsLocked("one", async () => {
      expect(view.state.readOnly).toBe(true);
      expect(view.state.facet(EditorView.editable)).toBe(false);
      expect(other.state.readOnly).toBe(false);
      view.dispatch({ changes: { from: 0, insert: "Unsafe typing" } });
      expect(view.state.doc.toString()).toBe("Original");
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: "Rewritten" },
        annotations: Transaction.addToHistory.of(false),
      });
      expect(view.state.doc.toString()).toBe("Rewritten");
    });
    expect(view.state.readOnly).toBe(false);
    view.dispatch({ changes: { from: 0, insert: "New " } });
    expect(view.state.doc.toString()).toBe("New Rewritten");
  });

  it("keeps nested locks until the outer operation finishes and releases them on failure", async () => {
    const view = editor("one");
    await expect(withPageEditorsLocked("one", async () => {
      await withPageEditorsLocked("one", async () => {
        expect(view.state.readOnly).toBe(true);
      });
      expect(view.state.readOnly).toBe(true);
      throw new Error("Apply failed");
    })).rejects.toThrow("Apply failed");
    expect(view.state.readOnly).toBe(false);
  });

  it("locks editors mounted during application and unlocks restored cached states", async () => {
    let cached: EditorState | undefined;
    await withPageEditorsLocked("one", async () => {
      const view = editor("one");
      expect(view.state.readOnly).toBe(true);
      cached = view.state;
      view.destroy();
      views.splice(views.indexOf(view), 1);
    });
    const restored = editor("one", cached);
    await Promise.resolve();
    expect(restored.state.readOnly).toBe(false);
  });

  it("awaits all source reloads before releasing the edit lock", async () => {
    const view = editor("reload");
    const other = vi.fn();
    unregister.push(registerPageEditorReload("other", other));
    unregister.push(registerPageEditorReload("reload", async () => {
      expect(view.state.readOnly).toBe(true);
      await Promise.resolve();
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: "Original[^grafium-note-1]" },
        annotations: Transaction.addToHistory.of(false),
      });
    }));
    await withPageEditorsLocked("reload", () => reloadPageEditors("reload"));
    expect(view.state.doc.toString()).toBe("Original[^grafium-note-1]");
    expect(view.state.readOnly).toBe(false);
    expect(other).not.toHaveBeenCalled();
  });

  it("keeps a stale editor locked after reload failure and recovers before the next flush", async () => {
    const view = editor("stale");
    const reload = vi.fn().mockRejectedValueOnce(new Error("Read failed")).mockResolvedValue(undefined);
    const flush = vi.fn();
    unregister.push(registerPageEditorReload("stale", reload), registerEditorFlush("stale", flush));
    await expect(withPageEditorsLocked("stale", () => reloadPageEditors("stale"))).rejects.toThrow("Read failed");
    expect(view.state.readOnly).toBe(true);
    view.dispatch({ changes: { from: 0, insert: "Unsafe" } });
    expect(view.state.doc.toString()).toBe("Original");
    await withPageEditorsLocked("stale", () => flushPageEditors("stale"));
    expect(reload).toHaveBeenCalledTimes(2);
    expect(flush).toHaveBeenCalledOnce();
    expect(view.state.readOnly).toBe(false);
  });
});
