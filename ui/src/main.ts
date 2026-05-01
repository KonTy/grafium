import "./styles/global.css";
import App from "./App.svelte";
import { mount } from "svelte";
import { undo, redo } from "@codemirror/commands";

// === Native undo/redo handlers (called by Rust via eval()) ===
// These are set as globals so Rust can call them directly
(window as any).__handleNativeUndo = () => {
  console.log("[undo] native handler called, activeView:", !!(window as any).__activeEditorView);
  const view = (window as any).__activeEditorView;
  if (view) {
    // Try CodeMirror undo first. If it returns false, nothing was undone
    // in the text editor, so fall through to app-level undo
    const didUndo = undo(view);
    console.log("[undo] CodeMirror undo result:", didUndo);
    if (!didUndo) {
      window.dispatchEvent(new CustomEvent("app-undo"));
    }
  } else {
    window.dispatchEvent(new CustomEvent("app-undo"));
  }
};

(window as any).__handleNativeRedo = () => {
  console.log("[redo] native handler called");
  const view = (window as any).__activeEditorView;
  if (view) {
    const didRedo = redo(view);
    if (!didRedo) {
      window.dispatchEvent(new CustomEvent("app-redo"));
    }
  } else {
    window.dispatchEvent(new CustomEvent("app-redo"));
  }
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

const app = mount(App, { target: document.getElementById("app")! });

export default app;
