/**
 * The Cut / Copy / Paste / Select all part of the context menu.
 *
 * WebKit's own menu used to supply these; the app now suppresses it, so this
 * stands in for it and picks what to offer from what was right-clicked:
 * a text field, a CodeMirror editor, other editable content, or read-only text
 * that is selected.
 *
 * Copy and Cut run `execCommand` inside the click, which is what the webview
 * accepts as a user gesture, and which makes an editor's own copy and cut
 * handlers run just as they do for the keyboard shortcut. Paste is different:
 * a page may not read the clipboard without a permission prompt, so on
 * desktop the webview is asked to paste by itself (`nativePaste`), exactly as
 * Ctrl+V would. The page reads the clipboard only where that command does not
 * exist.
 */
import { selectAll } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { nativePaste } from "./api";
import { writeClipboardText } from "./blockClipboard";
import {
  EDIT_SECTION_ORDER,
  type ContextMenuAction,
  type ContextMenuSection,
} from "./contextMenuSections";
import { focusElement } from "./focusElement";
import { formatBinding } from "./shortcuts";
import { showToast } from "./toast.svelte";

type TextField = HTMLInputElement | HTMLTextAreaElement;

export type EditContext =
  | { kind: "field"; field: TextField }
  | { kind: "editor"; view: EditorView }
  | { kind: "editable"; root: HTMLElement }
  | { kind: "selection"; text: string };

interface Point {
  x: number;
  y: number;
}

/** Input types that hold text; the others (checkboxes, sliders, ...) have nothing to cut or paste. */
const TEXT_INPUT_TYPES = new Set(["text", "search", "url", "tel", "email", "password", "number"]);

function fieldSelection(field: TextField): { start: number; end: number } | null {
  try {
    const { selectionStart: start, selectionEnd: end } = field;
    return start === null || end === null ? null : { start, end };
  } catch {
    return null;
  }
}

function caretAt({ x, y }: Point): { node: Node; offset: number } | null {
  const doc = document as Document & {
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  if (typeof doc.caretPositionFromPoint === "function") {
    const position = doc.caretPositionFromPoint(x, y);
    if (position) return { node: position.offsetNode, offset: position.offset };
  }
  if (typeof doc.caretRangeFromPoint === "function") {
    const range = doc.caretRangeFromPoint(x, y);
    if (range) return { node: range.startContainer, offset: range.startOffset };
  }
  return null;
}

/** Selected read-only text, but only when the pointer is on it, as in a native menu. */
function selectedTextAt(target: Element, point: Point): string {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return "";
  const text = selection.toString();
  if (!text.trim()) return "";
  try {
    const hit = caretAt(point);
    if (hit) return selection.getRangeAt(0).isPointInRange(hit.node, hit.offset) ? text : "";
    return selection.containsNode(target, true) ? text : "";
  } catch {
    return "";
  }
}

export function resolveEditContext(target: Element, point: Point): EditContext | null {
  const field = target.closest("input, textarea");
  if (field instanceof HTMLTextAreaElement) return { kind: "field", field };
  if (field instanceof HTMLInputElement) {
    return TEXT_INPUT_TYPES.has(field.type) ? { kind: "field", field } : null;
  }

  const host = target.closest<HTMLElement>(".cm-editor");
  const view = host ? EditorView.findFromDOM(host) : null;
  if (view) return { kind: "editor", view };

  const root = target.closest<HTMLElement>("[contenteditable]:not([contenteditable='false'])");
  if (root) return { kind: "editable", root };

  const text = selectedTextAt(target, point);
  return text ? { kind: "selection", text } : null;
}

interface EditState {
  editable: boolean;
  /** A password: nothing may be copied out of it. */
  secret: boolean;
  selected: boolean;
  hasContent: boolean;
}

function describe(context: EditContext): EditState {
  switch (context.kind) {
    case "field": {
      const { field } = context;
      const range = fieldSelection(field);
      return {
        editable: !field.readOnly && !field.disabled,
        secret: field instanceof HTMLInputElement && field.type === "password",
        // A number input does not report its selection; offer the actions when it has a value.
        selected: range ? range.start !== range.end : field.value.length > 0,
        hasContent: field.value.length > 0,
      };
    }
    case "editor": {
      const { state } = context.view;
      return {
        editable: state.facet(EditorView.editable) && !state.readOnly,
        secret: false,
        selected: state.selection.ranges.some((range) => !range.empty),
        hasContent: state.doc.length > 0,
      };
    }
    case "editable":
      return {
        editable: true,
        secret: false,
        selected: !(window.getSelection()?.isCollapsed ?? true),
        hasContent: (context.root.textContent ?? "") !== "",
      };
    case "selection":
      return { editable: false, secret: false, selected: true, hasContent: true };
  }
}

function selectedText(context: EditContext): string {
  switch (context.kind) {
    case "field": {
      const range = fieldSelection(context.field);
      const { value } = context.field;
      return range ? value.slice(range.start, range.end) : value;
    }
    case "editor": {
      const { state } = context.view;
      return state.selection.ranges
        .filter((range) => !range.empty)
        .map((range) => state.sliceDoc(range.from, range.to))
        .join(state.lineBreak);
    }
    case "editable":
      return window.getSelection()?.toString() ?? "";
    case "selection":
      return context.text;
  }
}

function runCommand(command: string, value?: string): boolean {
  try {
    return typeof document.execCommand === "function" && document.execCommand(command, false, value);
  } catch {
    return false;
  }
}

function focusContext(context: EditContext): void {
  switch (context.kind) {
    case "field":
      focusElement(context.field);
      break;
    case "editor":
      context.view.focus();
      break;
    case "editable":
      focusElement(context.root);
      break;
    case "selection":
      break;
  }
}

function replaceFieldSelection(field: TextField, text: string): void {
  focusElement(field);
  if (text ? runCommand("insertText", text) : runCommand("delete")) return;
  const range = fieldSelection(field);
  if (range) field.setRangeText(text, range.start, range.end, "end");
  else field.value = text;
  field.dispatchEvent(
    new InputEvent("input", {
      bubbles: true,
      inputType: text ? "insertFromPaste" : "deleteByCut",
      data: text || null,
    }),
  );
}

function deleteSelection(context: EditContext): void {
  switch (context.kind) {
    case "field":
      replaceFieldSelection(context.field, "");
      break;
    case "editor":
      context.view.dispatch(context.view.state.replaceSelection(""), {
        userEvent: "delete.cut",
        scrollIntoView: true,
      });
      break;
    case "editable":
      runCommand("delete");
      break;
    case "selection":
      break;
  }
}

async function copySelection(context: EditContext): Promise<void> {
  if (context.kind !== "selection") {
    focusContext(context);
    if (runCommand("copy")) return;
  }
  await writeClipboardText(selectedText(context));
}

async function cutSelection(context: EditContext): Promise<void> {
  focusContext(context);
  if (runCommand("cut")) return;
  await writeClipboardText(selectedText(context));
  focusContext(context);
  deleteSelection(context);
}

interface ClipboardContent {
  text: string;
  html: string;
  files: File[];
}

/** Null when the page may not read the clipboard at all. */
async function readClipboard(): Promise<ClipboardContent | null> {
  const clipboard = navigator.clipboard;
  try {
    if (typeof clipboard?.read === "function") {
      const content: ClipboardContent = { text: "", html: "", files: [] };
      for (const item of await clipboard.read()) {
        for (const type of item.types) {
          const blob = await item.getType(type);
          if (type === "text/plain") content.text = await blob.text();
          else if (type === "text/html") content.html = await blob.text();
          else if (type.startsWith("image/")) {
            content.files.push(new File([blob], `pasted.${type.slice("image/".length)}`, { type }));
          }
        }
      }
      return content;
    }
  } catch {
    // Fall back to plain text below.
  }
  try {
    if (typeof clipboard?.readText === "function") {
      return { text: await clipboard.readText(), html: "", files: [] };
    }
  } catch {
    // Reading is not allowed here.
  }
  return null;
}

function pasteIntoEditor(view: EditorView, content: ClipboardContent): void {
  let handled = false;
  try {
    // The editors turn pasted HTML and images into Markdown in their own paste
    // handlers, so give them what a real paste would.
    const data = new DataTransfer();
    if (content.text) data.setData("text/plain", content.text);
    if (content.html) data.setData("text/html", content.html);
    for (const file of content.files) data.items.add(file);
    const event = new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true });
    view.contentDOM.dispatchEvent(event);
    handled = event.defaultPrevented;
  } catch {
    // This webview cannot build clipboard events; insert the text directly.
  }
  if (!handled && content.text) {
    view.dispatch(view.state.replaceSelection(content.text), {
      userEvent: "input.paste",
      scrollIntoView: true,
    });
  }
}

async function pasteClipboard(context: EditContext): Promise<void> {
  focusContext(context);
  try {
    if ((await nativePaste()) === "requested") return;
  } catch {
    // Not a webview that can paste for us; read the clipboard instead.
  }

  const content = await readClipboard();
  if (!content) {
    showToast(`Paste is blocked here. Press ${formatBinding("mod+v")} instead.`, "info");
    return;
  }
  if (!content.text && !content.html && content.files.length === 0) return;

  focusContext(context);
  switch (context.kind) {
    case "editor":
      pasteIntoEditor(context.view, content);
      break;
    case "field":
      replaceFieldSelection(context.field, content.text);
      break;
    case "editable":
      runCommand("insertText", content.text);
      break;
    case "selection":
      break;
  }
}

function selectEverything(context: EditContext): void {
  focusContext(context);
  switch (context.kind) {
    case "field":
      context.field.select();
      break;
    case "editor":
      selectAll(context.view);
      break;
    case "editable":
      runCommand("selectAll");
      break;
    case "selection":
      break;
  }
}

/**
 * The clipboard actions that make sense for what was right-clicked, or null
 * when it holds no text. `point` is the pointer in viewport pixels.
 */
export function buildEditSection(target: Element, point: Point = { x: 0, y: 0 }): ContextMenuSection | null {
  const context = resolveEditContext(target, point);
  if (!context) return null;
  const state = describe(context);
  const items: ContextMenuAction[] = [];

  if (state.editable && !state.secret) {
    items.push({
      id: "edit.cut",
      label: "Cut",
      hint: formatBinding("mod+x"),
      disabled: !state.selected,
      onSelect: () => cutSelection(context),
    });
  }
  if (!state.secret) {
    items.push({
      id: "edit.copy",
      label: "Copy",
      hint: formatBinding("mod+c"),
      disabled: !state.selected,
      onSelect: () => copySelection(context),
    });
  }
  if (state.editable) {
    items.push({
      id: "edit.paste",
      label: "Paste",
      hint: formatBinding("mod+v"),
      onSelect: () => pasteClipboard(context),
    });
  }
  if (context.kind !== "selection") {
    items.push({
      id: "edit.selectAll",
      label: "Select all",
      hint: formatBinding("mod+a"),
      disabled: !state.hasContent,
      onSelect: () => selectEverything(context),
    });
  }

  return items.length > 0 ? { id: "edit", ariaLabel: "Edit", order: EDIT_SECTION_ORDER, items } : null;
}
