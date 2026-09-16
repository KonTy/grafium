import "./styles/global.css";
import "katex/dist/katex.min.css";
import App from "./App.svelte";
import { mount } from "svelte";
import { EditorView } from "@codemirror/view";
import { redoEditor as redo, undoEditor as undo } from "./lib/editorUndo";
import { canRedo, canUndo } from "./lib/undoStack";

function activeEditorView(): EditorView | null {
  const view = (window as any).__activeEditorView;
  return view instanceof EditorView ? view : null;
}

function editorHasDomFocus(view: EditorView): boolean {
  const active = document.activeElement;
  return view.hasFocus || (!!active && view.dom.contains(active));
}

// === Native undo/redo handlers (called by Rust via eval()) ===
// These are set as globals so Rust can call them directly
(window as any).__handleNativeUndo = () => {
  const view = activeEditorView();
  const editorFocused = view ? editorHasDomFocus(view) : false;
  console.log("[undo] native handler called, activeView:", !!view, "editorFocused:", editorFocused);
  if (view && editorFocused) {
    // Try CodeMirror undo first. If it returns false, nothing was undone
    // in the text editor, so fall through to app-level undo
    const didUndo = undo(view);
    console.log("[undo] CodeMirror undo result:", didUndo);
    if (!didUndo) {
      window.dispatchEvent(new CustomEvent("app-undo"));
    }
    return;
  }
  if (canUndo()) {
    window.dispatchEvent(new CustomEvent("app-undo"));
    return;
  }
  if (view) {
    const didUndo = undo(view);
    console.log("[undo] unfocused CodeMirror undo fallback result:", didUndo);
    if (didUndo) return;
  }
  window.dispatchEvent(new CustomEvent("app-undo"));
};

(window as any).__handleNativeRedo = () => {
  const view = activeEditorView();
  const editorFocused = view ? editorHasDomFocus(view) : false;
  console.log("[redo] native handler called, activeView:", !!view, "editorFocused:", editorFocused);
  if (view && editorFocused) {
    const didRedo = redo(view);
    if (!didRedo) {
      window.dispatchEvent(new CustomEvent("app-redo"));
    }
    return;
  }
  if (canRedo()) {
    window.dispatchEvent(new CustomEvent("app-redo"));
    return;
  }
  if (view) {
    const didRedo = redo(view);
    if (didRedo) return;
  }
  window.dispatchEvent(new CustomEvent("app-redo"));
};

(window as any).__handleNativeVerticalArrow = (direction: "up" | "down", extend: boolean) => {
  // GTK intercepts Shift+Arrow before WebKit. Reuse the focused editor/feed's
  // handlers instead of moving a cached (possibly unfocused) CodeMirror view.
  const target = document.activeElement;
  if (!(target instanceof HTMLElement) || EditorView.findFromDOM(target)?.composing) return false;
  const key = direction === "up" ? "ArrowUp" : "ArrowDown";
  return !target.dispatchEvent(new KeyboardEvent("keydown", {
    key, code: key, shiftKey: extend, bubbles: true, cancelable: true,
  }));
};

// === Fallback: beforeinput event ===
// On WebKitGTK, even when keydown is swallowed, beforeinput fires
// with inputType "historyUndo"/"historyRedo" on contenteditable elements
document.addEventListener("beforeinput", (e: Event) => {
  const inputEvent = e as InputEvent;
  if (inputEvent.inputType === "historyUndo") {
    console.log("[undo] beforeinput historyUndo caught");
    inputEvent.preventDefault();
    (window as any).__handleNativeUndo();
  } else if (inputEvent.inputType === "historyRedo") {
    console.log("[redo] beforeinput historyRedo caught");
    inputEvent.preventDefault();
    (window as any).__handleNativeRedo();
  }
}, true); // capture phase

// === Fallback: keydown event ===
// In case keydown does reach JS (when no contenteditable is focused)
document.addEventListener("keydown", (e: KeyboardEvent) => {
  if (e.ctrlKey && !e.shiftKey && e.key === "z") {
    console.log("[undo] keydown caught");
    e.preventDefault();
    (window as any).__handleNativeUndo();
  } else if (e.ctrlKey && e.shiftKey && e.key === "z") {
    console.log("[redo] keydown caught");
    e.preventDefault();
    (window as any).__handleNativeRedo();
  } else if (e.ctrlKey && e.key === "y") {
    console.log("[redo] keydown Ctrl+Y caught");
    e.preventDefault();
    (window as any).__handleNativeRedo();
  }
}, true); // capture phase

// === Toggle reference panel (called by Rust via eval() for Ctrl+.) ===
(window as any).__toggleReferencePanel = () => {
  window.dispatchEvent(new CustomEvent("toggle-reference-panel"));
};

const app = mount(App, { target: document.getElementById("app")! });
window.dispatchEvent(new Event("grafium-ready"));

// === TEMP DIAGNOSTIC: key-event visibility on WebKitGTK ===
// Shows what the JS layer actually receives when arrow keys are pressed.
{
  const dbg = document.createElement("div");
  dbg.id = "__keydbg";
  dbg.style.cssText =
    "position:fixed;bottom:8px;right:8px;z-index:99999;background:#000;color:#0f0;" +
    "font:11px/1.4 monospace;padding:6px 8px;border:1px solid #0f0;pointer-events:none;" +
    "max-width:360px;white-space:pre;opacity:0.9;border-radius:4px;display:none;";
  dbg.textContent = "keydbg: press ↑/↓ in a block";
  document.body.appendChild(dbg);
  let dbgVisible = false;
  const lines: string[] = [];
  (window as any).__keydbg = (msg: string) => {
    if (!dbgVisible) return;
    lines.unshift(msg);
    if (lines.length > 6) lines.pop();
    dbg.textContent = lines.join("\n");
  };
  // Toggle the diagnostic overlay with Ctrl+Shift+D.
  document.addEventListener(
    "keydown",
    (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && (e.key === "D" || e.key === "d")) {
        e.preventDefault();
        dbgVisible = !dbgVisible;
        dbg.style.display = dbgVisible ? "block" : "none";
      }
    },
    true,
  );
  document.addEventListener(
    "keydown",
    (e: KeyboardEvent) => {
      // Cheap early-out when the debug box is hidden (the default), so this
      // adds zero per-keystroke cost during normal use.
      if (!dbgVisible) return;
      if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
      const t = e.target as HTMLElement | null;
      (window as any).__keydbg(
        `DOC ${e.key} tgt=${t?.tagName ?? "?"} CE=${!!t?.isContentEditable}`,
      );
    },
    true,
  );
}

export default app;
