import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorSelection, EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

const nativePaste = vi.hoisted(() => vi.fn());
const writeClipboardText = vi.hoisted(() => vi.fn());
vi.mock("./api", () => ({ nativePaste }));
vi.mock("./blockClipboard", () => ({ writeClipboardText }));

import { buildEditSection, resolveEditContext } from "./editActions";
import { EDIT_SECTION_ORDER, type ContextMenuAction, type ContextMenuSection } from "./contextMenuSections";
import { formatBinding } from "./shortcuts";
import { toasts } from "./toast.svelte";

const point = { x: 5, y: 5 };

function actions(section: ContextMenuSection | null): ContextMenuAction[] {
  return (section?.items ?? []) as ContextMenuAction[];
}

const idsOf = (section: ContextMenuSection | null) => actions(section).map((item) => item.id);

function item(section: ContextMenuSection | null, id: string): ContextMenuAction {
  const found = actions(section).find((candidate) => candidate.id === id);
  if (!found) throw new Error(`no ${id} in ${JSON.stringify(idsOf(section))}`);
  return found;
}

function textField(value: string, selection?: [number, number], type = "text"): HTMLInputElement {
  const input = document.createElement("input");
  input.type = type;
  input.value = value;
  document.body.append(input);
  if (selection) input.setSelectionRange(selection[0], selection[1]);
  return input;
}

const views: EditorView[] = [];

function editor(doc: string, selection?: { anchor: number; head?: number }, extensions: Extension[] = []) {
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({ doc, selection, extensions }),
  });
  views.push(view);
  return view;
}

/** Read-only text with its whole content selected and the pointer on it. */
function selectedParagraph(text: string): HTMLParagraphElement {
  const p = document.createElement("p");
  p.textContent = text;
  document.body.append(p);
  const range = document.createRange();
  range.selectNodeContents(p);
  const selection = window.getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
  Object.defineProperty(document, "caretPositionFromPoint", {
    value: () => ({ offsetNode: p.firstChild, offset: 1 }),
    configurable: true,
  });
  return p;
}

let execCommand: ReturnType<typeof vi.fn>;
let clipboard: { read?: ReturnType<typeof vi.fn>; readText?: ReturnType<typeof vi.fn> };

function setClipboard(value: typeof clipboard | undefined) {
  Object.defineProperty(navigator, "clipboard", { value, configurable: true });
}

beforeEach(() => {
  document.body.innerHTML = "";
  nativePaste.mockReset();
  nativePaste.mockResolvedValue("unsupported");
  writeClipboardText.mockReset();
  writeClipboardText.mockResolvedValue(undefined);
  toasts.splice(0, toasts.length);
  execCommand = vi.fn(() => true);
  Object.defineProperty(document, "execCommand", { value: execCommand, configurable: true, writable: true });
  clipboard = { readText: vi.fn(async () => "") };
  setClipboard(clipboard);
  window.getSelection()?.removeAllRanges();
});

afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  setClipboard(undefined);
  Reflect.deleteProperty(document, "caretPositionFromPoint");
  Reflect.deleteProperty(document, "caretRangeFromPoint");
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("resolveEditContext", () => {
  it("finds text fields", () => {
    const input = textField("x");
    const area = document.createElement("textarea");
    document.body.append(area);
    expect(resolveEditContext(input, point)).toEqual({ kind: "field", field: input });
    expect(resolveEditContext(area, point)).toEqual({ kind: "field", field: area });
  });

  it("treats every text-like input type as a field", () => {
    for (const type of ["text", "search", "url", "tel", "email", "password", "number"]) {
      expect(resolveEditContext(textField("1", undefined, type), point)?.kind).toBe("field");
    }
  });

  it("has nothing to offer on checkboxes, sliders and buttons", () => {
    for (const type of ["checkbox", "radio", "range", "button", "color", "file"]) {
      const input = document.createElement("input");
      input.type = type;
      document.body.append(input);
      expect(resolveEditContext(input, point)).toBeNull();
    }
  });

  it("finds a CodeMirror editor from anywhere inside it", () => {
    const view = editor("hello");
    expect(resolveEditContext(view.contentDOM, point)).toEqual({ kind: "editor", view });
    expect(resolveEditContext(view.dom, point)).toEqual({ kind: "editor", view });
  });

  it("finds other editable content, but not what opts out", () => {
    const root = document.createElement("div");
    root.setAttribute("contenteditable", "true");
    const child = document.createElement("span");
    root.append(child);
    const locked = document.createElement("div");
    locked.setAttribute("contenteditable", "false");
    document.body.append(root, locked);
    expect(resolveEditContext(child, point)).toEqual({ kind: "editable", root });
    expect(resolveEditContext(locked, point)).toBeNull();
  });

  it("has nothing to offer on text that is not selected", () => {
    const p = document.createElement("p");
    p.textContent = "plain text";
    document.body.append(p);
    expect(resolveEditContext(p, point)).toBeNull();
  });

  describe("selected read-only text", () => {
    function select(el: Element) {
      const range = document.createRange();
      range.selectNodeContents(el);
      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);
    }

    it("is offered when the pointer is on the selection", () => {
      const p = document.createElement("p");
      p.textContent = "selected words";
      document.body.append(p);
      select(p);
      Object.defineProperty(document, "caretPositionFromPoint", {
        value: () => ({ offsetNode: p.firstChild, offset: 3 }),
        configurable: true,
      });
      expect(resolveEditContext(p, point)).toEqual({ kind: "selection", text: "selected words" });
    });

    it("is not offered when the pointer is elsewhere", () => {
      const p = document.createElement("p");
      p.textContent = "selected words";
      const other = document.createElement("p");
      other.textContent = "somewhere else";
      document.body.append(p, other);
      select(p);
      Object.defineProperty(document, "caretPositionFromPoint", {
        value: () => ({ offsetNode: other.firstChild, offset: 3 }),
        configurable: true,
      });
      expect(resolveEditContext(other, point)).toBeNull();
    });

    it("uses the older caret API where that is all there is", () => {
      const p = document.createElement("p");
      p.textContent = "selected words";
      document.body.append(p);
      select(p);
      Object.defineProperty(document, "caretRangeFromPoint", {
        value: () => {
          const range = document.createRange();
          range.setStart(p.firstChild!, 2);
          return range;
        },
        configurable: true,
      });
      expect(resolveEditContext(p, point)?.kind).toBe("selection");
    });

    it("falls back to whether the target is part of the selection", () => {
      const p = document.createElement("p");
      p.textContent = "selected words";
      const other = document.createElement("p");
      other.textContent = "somewhere else";
      document.body.append(p, other);
      select(p);
      // jsdom's own containsNode does not implement partial containment.
      vi.spyOn(window.getSelection()!, "containsNode").mockImplementation((node) => p.contains(node));
      expect(resolveEditContext(p, point)?.kind).toBe("selection");
      expect(resolveEditContext(other, point)).toBeNull();
    });

    it("ignores a selection of nothing but whitespace", () => {
      const p = document.createElement("p");
      p.textContent = "   ";
      document.body.append(p);
      select(p);
      expect(resolveEditContext(p, point)).toBeNull();
    });
  });
});

describe("buildEditSection", () => {
  it("describes itself as the Edit group, last in the menu", () => {
    const section = buildEditSection(textField("hello", [0, 5]), point)!;
    expect(section.id).toBe("edit");
    expect(section.ariaLabel).toBe("Edit");
    expect(section.order).toBe(EDIT_SECTION_ORDER);
  });

  it("shows the keyboard shortcut beside each action", () => {
    const section = buildEditSection(textField("hello", [0, 5]), point);
    expect(item(section, "edit.cut").hint).toBe(formatBinding("mod+x"));
    expect(item(section, "edit.copy").hint).toBe(formatBinding("mod+c"));
    expect(item(section, "edit.paste").hint).toBe(formatBinding("mod+v"));
    expect(item(section, "edit.selectAll").hint).toBe(formatBinding("mod+a"));
  });

  it("is null where there is nothing to cut, copy, paste or select", () => {
    expect(buildEditSection(document.body, point)).toBeNull();
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    document.body.append(checkbox);
    expect(buildEditSection(checkbox, point)).toBeNull();
  });

  describe("in a text field", () => {
    it("offers everything, enabled, when text is selected", () => {
      const section = buildEditSection(textField("hello world", [0, 5]), point);
      expect(idsOf(section)).toEqual(["edit.cut", "edit.copy", "edit.paste", "edit.selectAll"]);
      expect(actions(section).map((a) => !!a.disabled)).toEqual([false, false, false, false]);
    });

    it("disables cut and copy when nothing is selected", () => {
      const section = buildEditSection(textField("hello world", [3, 3]), point);
      expect(item(section, "edit.cut").disabled).toBe(true);
      expect(item(section, "edit.copy").disabled).toBe(true);
      expect(item(section, "edit.paste").disabled).toBeFalsy();
      expect(item(section, "edit.selectAll").disabled).toBe(false);
    });

    it("disables select all when empty", () => {
      expect(item(buildEditSection(textField(""), point), "edit.selectAll").disabled).toBe(true);
    });

    it("offers neither cut nor copy for a password", () => {
      const section = buildEditSection(textField("hunter2", [0, 7], "password"), point);
      expect(idsOf(section)).toEqual(["edit.paste", "edit.selectAll"]);
    });

    it("offers only copy and select all for a read-only or disabled field", () => {
      const readonly = textField("hello", [0, 5]);
      readonly.readOnly = true;
      expect(idsOf(buildEditSection(readonly, point))).toEqual(["edit.copy", "edit.selectAll"]);
      const disabled = textField("hello", [0, 5]);
      disabled.disabled = true;
      expect(idsOf(buildEditSection(disabled, point))).toEqual(["edit.copy", "edit.selectAll"]);
    });

    it("treats a number input with a value as selectable, since it reports no selection", () => {
      const filled = buildEditSection(textField("42", undefined, "number"), point);
      expect(item(filled, "edit.copy").disabled).toBe(false);
      const empty = buildEditSection(textField("", undefined, "number"), point);
      expect(item(empty, "edit.copy").disabled).toBe(true);
    });
  });

  describe("in a CodeMirror editor", () => {
    it("offers everything for a selection and disables cut and copy for a caret", () => {
      const selected = buildEditSection(editor("hello world", { anchor: 0, head: 5 }).contentDOM, point);
      expect(idsOf(selected)).toEqual(["edit.cut", "edit.copy", "edit.paste", "edit.selectAll"]);
      expect(item(selected, "edit.cut").disabled).toBe(false);

      const caret = buildEditSection(editor("hello world", { anchor: 3 }).contentDOM, point);
      expect(item(caret, "edit.cut").disabled).toBe(true);
      expect(item(caret, "edit.copy").disabled).toBe(true);
    });

    it("offers only copy and select all when it cannot be edited", () => {
      const readOnly = editor("hello", { anchor: 0, head: 5 }, [EditorState.readOnly.of(true)]);
      expect(idsOf(buildEditSection(readOnly.contentDOM, point))).toEqual(["edit.copy", "edit.selectAll"]);
      const locked = editor("hello", { anchor: 0, head: 5 }, [EditorView.editable.of(false)]);
      expect(idsOf(buildEditSection(locked.contentDOM, point))).toEqual(["edit.copy", "edit.selectAll"]);
    });

    it("disables select all for an empty document", () => {
      const section = buildEditSection(editor("").contentDOM, point);
      expect(item(section, "edit.selectAll").disabled).toBe(true);
    });
  });

  it("offers only copy for selected read-only text", () => {
    const p = selectedParagraph("read only");
    const section = buildEditSection(p, point);
    expect(idsOf(section)).toEqual(["edit.copy"]);
    expect(item(section, "edit.copy").disabled).toBeFalsy();
  });
});

describe("Copy", () => {
  it("copies through the webview, from the field the menu was opened on", async () => {
    const input = textField("hello world", [0, 5]);
    let focused: Element | null = null;
    execCommand.mockImplementation(() => {
      focused = document.activeElement;
      return true;
    });
    await item(buildEditSection(input, point), "edit.copy").onSelect();
    expect(execCommand).toHaveBeenCalledWith("copy", false, undefined);
    expect(focused).toBe(input);
    expect(writeClipboardText).not.toHaveBeenCalled();
  });

  it("writes the selected text itself when the webview refuses", async () => {
    execCommand.mockReturnValue(false);
    await item(buildEditSection(textField("hello world", [6, 11]), point), "edit.copy").onSelect();
    expect(writeClipboardText).toHaveBeenCalledWith("world");
  });

  it("writes it when the command throws", async () => {
    execCommand.mockImplementation(() => {
      throw new Error("not allowed");
    });
    await item(buildEditSection(textField("hello", [0, 5]), point), "edit.copy").onSelect();
    expect(writeClipboardText).toHaveBeenCalledWith("hello");
  });

  it("copies an editor's selection, joining several ranges with line breaks", async () => {
    execCommand.mockReturnValue(false);
    const view = editor("one two three", undefined, [EditorState.allowMultipleSelections.of(true)]);
    view.dispatch({
      selection: EditorSelection.create([EditorSelection.range(0, 3), EditorSelection.range(8, 13)]),
    });
    expect(view.state.selection.ranges).toHaveLength(2);
    await item(buildEditSection(view.contentDOM, point), "edit.copy").onSelect();
    expect(writeClipboardText).toHaveBeenCalledWith("one\nthree");
  });

  it("copies read-only text without using execCommand", async () => {
    const p = selectedParagraph("read only");
    await item(buildEditSection(p, point), "edit.copy").onSelect();
    expect(execCommand).not.toHaveBeenCalled();
    expect(writeClipboardText).toHaveBeenCalledWith("read only");
  });
});

describe("Cut", () => {
  it("cuts through the webview", async () => {
    await item(buildEditSection(textField("hello", [0, 5]), point), "edit.cut").onSelect();
    expect(execCommand).toHaveBeenCalledWith("cut", false, undefined);
    expect(writeClipboardText).not.toHaveBeenCalled();
  });

  it("copies and then deletes the selection when the webview refuses", async () => {
    execCommand.mockReturnValue(false);
    const input = textField("hello world", [0, 6]);
    const inputs: string[] = [];
    input.addEventListener("input", (event) => inputs.push((event as InputEvent).inputType));
    await item(buildEditSection(input, point), "edit.cut").onSelect();
    expect(writeClipboardText).toHaveBeenCalledWith("hello ");
    expect(input.value).toBe("world");
    expect(inputs).toEqual(["deleteByCut"]);
  });

  it("deletes an editor's selection when the webview refuses", async () => {
    execCommand.mockReturnValue(false);
    const view = editor("hello world", { anchor: 0, head: 6 });
    await item(buildEditSection(view.contentDOM, point), "edit.cut").onSelect();
    expect(writeClipboardText).toHaveBeenCalledWith("hello ");
    expect(view.state.doc.toString()).toBe("world");
  });

  it("does not delete anything if copying failed", async () => {
    execCommand.mockReturnValue(false);
    writeClipboardText.mockRejectedValue(new Error("denied"));
    const input = textField("hello", [0, 5]);
    await expect(item(buildEditSection(input, point), "edit.cut").onSelect()).rejects.toThrow("denied");
    expect(input.value).toBe("hello");
  });
});

describe("Paste", () => {
  it("asks the webview to paste, from the field the menu was opened on, and reads nothing itself", async () => {
    const input = textField("hello", [0, 0]);
    let focused: Element | null = null;
    nativePaste.mockImplementation(async () => {
      focused = document.activeElement;
      return "requested";
    });
    await item(buildEditSection(input, point), "edit.paste").onSelect();
    expect(focused).toBe(input);
    expect(clipboard.readText).not.toHaveBeenCalled();
    expect(execCommand).not.toHaveBeenCalled();
  });

  it("reads the clipboard itself where the webview cannot paste for it", async () => {
    clipboard.readText = vi.fn(async () => "PASTED");
    const input = textField("hello world", [6, 11]);
    execCommand.mockReturnValue(false);
    await item(buildEditSection(input, point), "edit.paste").onSelect();
    expect(input.value).toBe("hello PASTED");
    expect(input.selectionStart).toBe(12);
  });

  it("types it into the field when the webview allows that, so undo works", async () => {
    clipboard.readText = vi.fn(async () => "PASTED");
    await item(buildEditSection(textField("hello", [0, 5]), point), "edit.paste").onSelect();
    expect(execCommand).toHaveBeenCalledWith("insertText", false, "PASTED");
  });

  it("falls back the same way when asking the webview fails", async () => {
    nativePaste.mockRejectedValue(new Error("no such command"));
    clipboard.readText = vi.fn(async () => "PASTED");
    execCommand.mockReturnValue(false);
    const input = textField("", [0, 0]);
    await item(buildEditSection(input, point), "edit.paste").onSelect();
    expect(input.value).toBe("PASTED");
  });

  it("prefers the richer clipboard read and uses its text for a field", async () => {
    const blob = new Blob(["from read"], { type: "text/plain" });
    clipboard.read = vi.fn(async () => [{ types: ["text/plain"], getType: async () => blob }]);
    execCommand.mockReturnValue(false);
    const input = textField("", [0, 0]);
    await item(buildEditSection(input, point), "edit.paste").onSelect();
    expect(clipboard.read).toHaveBeenCalled();
    expect(clipboard.readText).not.toHaveBeenCalled();
    expect(input.value).toBe("from read");
  });

  it("tells the user to press the shortcut when the clipboard cannot be read", async () => {
    clipboard.readText = vi.fn(async () => {
      throw new DOMException("denied", "NotAllowedError");
    });
    const input = textField("keep", [0, 4]);
    await item(buildEditSection(input, point), "edit.paste").onSelect();
    expect(toasts.map((t) => [t.message, t.severity])).toEqual([
      [`Paste is blocked here. Press ${formatBinding("mod+v")} instead.`, "info"],
    ]);
    expect(input.value).toBe("keep");
  });

  it("tells the user the same when there is no clipboard at all", async () => {
    setClipboard(undefined);
    await item(buildEditSection(textField("x", [0, 0]), point), "edit.paste").onSelect();
    expect(toasts).toHaveLength(1);
  });

  it("does nothing for an empty clipboard", async () => {
    clipboard.readText = vi.fn(async () => "");
    const input = textField("keep", [0, 4]);
    await item(buildEditSection(input, point), "edit.paste").onSelect();
    expect(input.value).toBe("keep");
    expect(toasts).toHaveLength(0);
  });

  describe("into an editor", () => {
    class FakeDataTransfer {
      data = new Map<string, string>();
      items = { add: vi.fn() };
      setData(type: string, value: string) {
        this.data.set(type, value);
      }
      getData(type: string) {
        return this.data.get(type) ?? "";
      }
    }
    class FakeClipboardEvent extends Event {
      clipboardData: FakeDataTransfer;
      constructor(type: string, init: { clipboardData: FakeDataTransfer } & EventInit) {
        super(type, init);
        this.clipboardData = init.clipboardData;
      }
    }

    function richClipboard() {
      const html = new Blob(["<b>x</b>"], { type: "text/html" });
      const text = new Blob(["x"], { type: "text/plain" });
      clipboard.read = vi.fn(async () => [
        { types: ["text/plain", "text/html"], getType: async (type: string) => (type === "text/html" ? html : text) },
      ]);
    }

    it("replaces the selection with the text when no paste event can be made", async () => {
      clipboard.readText = vi.fn(async () => "NEW");
      const view = editor("hello world", { anchor: 6, head: 11 });
      await item(buildEditSection(view.contentDOM, point), "edit.paste").onSelect();
      expect(view.state.doc.toString()).toBe("hello NEW");
      expect(view.state.selection.main.head).toBe(9);
    });

    it("hands the editor's own paste handlers the HTML as well as the text", async () => {
      vi.stubGlobal("DataTransfer", FakeDataTransfer);
      vi.stubGlobal("ClipboardEvent", FakeClipboardEvent);
      richClipboard();
      const seen: Array<[string, string]> = [];
      const view = editor("abc", { anchor: 3 }, [
        EditorView.domEventHandlers({
          paste(event) {
            const data = (event as unknown as FakeClipboardEvent).clipboardData;
            seen.push([data.getData("text/plain"), data.getData("text/html")]);
            return true;
          },
        }),
      ]);
      await item(buildEditSection(view.contentDOM, point), "edit.paste").onSelect();

      expect(seen).toEqual([["x", "<b>x</b>"]]);
      expect(view.state.doc.toString()).toBe("abc");
    });

    it("pastes once when the editor's built-in handling takes the event", async () => {
      vi.stubGlobal("DataTransfer", FakeDataTransfer);
      vi.stubGlobal("ClipboardEvent", FakeClipboardEvent);
      richClipboard();
      const view = editor("abc", { anchor: 3 });
      await item(buildEditSection(view.contentDOM, point), "edit.paste").onSelect();
      expect(view.state.doc.toString()).toBe("abcx");
    });

    it("inserts the text itself when the clipboard event cannot be built", async () => {
      vi.stubGlobal("DataTransfer", FakeDataTransfer);
      vi.stubGlobal(
        "ClipboardEvent",
        class {
          constructor() {
            throw new TypeError("Illegal constructor");
          }
        },
      );
      clipboard.readText = vi.fn(async () => "xyz");
      const view = editor("abc", { anchor: 3 });
      await item(buildEditSection(view.contentDOM, point), "edit.paste").onSelect();
      expect(view.state.doc.toString()).toBe("abcxyz");
    });
  });

  it("types into other editable content", async () => {
    const root = document.createElement("div");
    root.setAttribute("contenteditable", "true");
    root.tabIndex = 0;
    document.body.append(root);
    clipboard.readText = vi.fn(async () => "PASTED");
    await item(buildEditSection(root, point), "edit.paste").onSelect();
    expect(execCommand).toHaveBeenCalledWith("insertText", false, "PASTED");
  });
});

describe("Select all", () => {
  it("selects a field's text", async () => {
    const input = textField("hello world", [2, 2]);
    await item(buildEditSection(input, point), "edit.selectAll").onSelect();
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 11]);
    expect(document.activeElement).toBe(input);
  });

  it("selects an editor's whole document", async () => {
    const view = editor("one\ntwo", { anchor: 1 });
    await item(buildEditSection(view.contentDOM, point), "edit.selectAll").onSelect();
    expect(view.state.selection.main.from).toBe(0);
    expect(view.state.selection.main.to).toBe(7);
  });

  it("selects other editable content through the webview", async () => {
    const root = document.createElement("div");
    root.setAttribute("contenteditable", "true");
    root.textContent = "words";
    document.body.append(root);
    await item(buildEditSection(root, point), "edit.selectAll").onSelect();
    expect(execCommand).toHaveBeenCalledWith("selectAll", false, undefined);
  });
});
