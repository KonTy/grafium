import { afterEach, describe, expect, it } from "vitest";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { history, undo } from "@codemirror/commands";

import { rewriteEditedBlock } from "./editedBlock";
import { bulletToTodoContent, taskToBulletContent } from "./taskSyntax";

type Change = [from: number, to: number, insert: string];

const views: EditorView[] = [];
let changes: Change[] = [];

/** An editor with undo history, as the block editor has, that records what was changed. */
function editor(doc: string, anchor = 0, head = anchor): EditorView {
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({
      doc,
      selection: EditorSelection.range(anchor, head),
      extensions: [
        history(),
        EditorView.updateListener.of((update) => {
          update.changes.iterChanges((from, to, _fromB, _toB, inserted) => {
            changes.push([from, to, inserted.toString()]);
          });
        }),
      ],
    }),
  });
  views.push(view);
  return view;
}

afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.innerHTML = "";
  changes = [];
});

const text = (view: EditorView) => view.state.doc.toString();

describe("rewriteEditedBlock", () => {
  it("replaces only the part of the text that differs", () => {
    const view = editor("hello big world");
    expect(rewriteEditedBlock(view, "hello small world")).toBe(true);
    expect(text(view)).toBe("hello small world");
    expect(changes).toEqual([[6, 9, "small"]]);
  });

  it("prefixes a bullet with TODO", () => {
    const view = editor("buy milk", 8);
    expect(rewriteEditedBlock(view, bulletToTodoContent("buy milk"))).toBe(true);
    expect(text(view)).toBe("TODO buy milk");
    expect(changes).toEqual([[0, 0, "TODO "]]);
  });

  it("turns a task back into a bullet", () => {
    const view = editor("TODO buy milk", 13);
    expect(rewriteEditedBlock(view, taskToBulletContent("TODO buy milk"))).toBe(true);
    expect(text(view)).toBe("buy milk");
    expect(changes).toEqual([[0, 5, ""]]);
  });

  it("keeps the caret on the same text when text is added before it", () => {
    const view = editor("buy milk", 4);
    rewriteEditedBlock(view, "TODO buy milk");
    const { head } = view.state.selection.main;
    expect(head).toBe(9);
    expect(view.state.doc.sliceString(head)).toBe("milk");
  });

  it("keeps a selection on the same text", () => {
    const view = editor("buy milk", 4, 8);
    rewriteEditedBlock(view, "TODO buy milk");
    const { from, to } = view.state.selection.main;
    expect(view.state.doc.sliceString(from, to)).toBe("milk");
  });

  it("leaves the caret alone when the change is after it", () => {
    const view = editor("buy milk", 2);
    rewriteEditedBlock(view, "buy milk now");
    expect(view.state.selection.main.head).toBe(2);
  });

  it("is reversed by one undo", () => {
    const view = editor("buy milk", 3);
    rewriteEditedBlock(view, "TODO buy milk");
    expect(text(view)).toBe("TODO buy milk");

    expect(undo(view)).toBe(true);
    expect(text(view)).toBe("buy milk");
    expect(undo(view)).toBe(false);
  });

  it("builds on text that is ahead of what is saved", () => {
    const view = editor("buy milk", 8);
    view.dispatch({ changes: { from: 8, insert: " and eggs" } });

    rewriteEditedBlock(view, bulletToTodoContent(text(view)));
    expect(text(view)).toBe("TODO buy milk and eggs");
  });

  it("does nothing when the text is already as asked", () => {
    const view = editor("TODO buy milk", 5);
    expect(rewriteEditedBlock(view, "TODO buy milk")).toBe(false);
    expect(changes).toEqual([]);
    expect(undo(view)).toBe(false);
  });

  it("does nothing once the editor is no longer on the page", () => {
    const view = editor("buy milk");
    view.dom.remove();

    expect(rewriteEditedBlock(view, "TODO buy milk")).toBe(false);
    expect(text(view)).toBe("buy milk");
    expect(changes).toEqual([]);
  });

  it("can empty the text and fill an empty editor", () => {
    const view = editor("buy milk");
    expect(rewriteEditedBlock(view, "")).toBe(true);
    expect(text(view)).toBe("");

    expect(rewriteEditedBlock(view, "TODO ")).toBe(true);
    expect(text(view)).toBe("TODO ");
  });

  it("handles text that is a repeat of itself", () => {
    const view = editor("aaaa");
    expect(rewriteEditedBlock(view, "aaaaaa")).toBe(true);
    expect(text(view)).toBe("aaaaaa");
    expect(rewriteEditedBlock(view, "aa")).toBe(true);
    expect(text(view)).toBe("aa");
  });

  it("keeps the line breaks of a multi-line block", () => {
    const view = editor("one\ntwo\nthree");
    expect(rewriteEditedBlock(view, "TODO one\ntwo\nthree")).toBe(true);
    expect(text(view)).toBe("TODO one\ntwo\nthree");
    expect(view.state.doc.lines).toBe(3);
    expect(changes).toEqual([[0, 0, "TODO "]]);
  });

  it("focuses the editor so typing carries on after the menu closes", () => {
    const view = editor("buy milk");
    rewriteEditedBlock(view, "TODO buy milk");
    expect(view.hasFocus).toBe(true);
  });

  describe("with characters outside the basic plane", () => {
    // U+1F600 and U+1F601 share a first UTF-16 unit; U+1F600 and U+1FA00 share a last one.
    const grin = "\u{1F600}";
    const beaming = "\u{1F601}";
    const neutralKing = "\u{1FA00}";

    function wholeCharacters(from: number, to: number, original: string) {
      expect(original.slice(from - 1, from)).not.toMatch(/[\ud800-\udbff]/);
      expect(original.slice(to, to + 1)).not.toMatch(/[\udc00-\udfff]/);
    }

    it("does not split a character whose first half is shared", () => {
      const original = `a${grin}b`;
      const view = editor(original);
      expect(rewriteEditedBlock(view, `a${beaming}b`)).toBe(true);
      expect(text(view)).toBe(`a${beaming}b`);
      expect(changes).toEqual([[1, 3, beaming]]);
      wholeCharacters(1, 3, original);
    });

    it("does not split a character whose second half is shared", () => {
      const original = `x${grin}y`;
      const view = editor(original);
      expect(rewriteEditedBlock(view, `x${neutralKing}y`)).toBe(true);
      expect(text(view)).toBe(`x${neutralKing}y`);
      expect(changes).toEqual([[1, 3, neutralKing]]);
      wholeCharacters(1, 3, original);
    });

    it("keeps a character that the change does not touch", () => {
      const view = editor(`${grin} buy milk`);
      expect(rewriteEditedBlock(view, `${grin} TODO buy milk`)).toBe(true);
      expect(text(view)).toBe(`${grin} TODO buy milk`);
      expect(changes).toEqual([[3, 3, "TODO "]]);
    });
  });
});
