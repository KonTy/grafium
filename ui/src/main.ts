import "./styles/global.css";
import "katex/dist/katex.min.css";
import App from "./App.svelte";
import { mount } from "svelte";
import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { invoke } from "@tauri-apps/api/core";
import {
  redo,
  undo,
} from "@codemirror/commands";

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

function getActiveEditorView(): EditorView | null {
  const globalView = (window as any).__activeEditorView ?? (window as any).__unifiedPageEditorView;
  if (globalView instanceof EditorView) {
    return globalView;
  }

  const active = document.activeElement as HTMLElement | null;
  if (!active) return null;

  const editorDom = active.closest(".cm-editor") as HTMLElement | null;
  return EditorView.findFromDOM(active) ?? (editorDom ? EditorView.findFromDOM(editorDom) : null);
}

function debugLog(message: string) {
  console.log(message);
  void invoke("debug_log", { message }).catch((error) => {
    console.error("[debug_log] failed", error);
  });
  (window as any).__keydbg?.(message);
}

function selectionSummary(view: EditorView): string {
  const selection = view.state.selection.main;
  const line = view.state.doc.lineAt(selection.head);
  return `anchor=${selection.anchor} head=${selection.head} empty=${selection.empty} line=${line.number}/${view.state.doc.lines} doc=${view.state.doc.length} focus=${view.hasFocus}`;
}

function moveVerticalSelection(view: EditorView, direction: "up" | "down", extend: boolean): boolean {
  view.focus();
  const selection = view.state.selection;
  const range = selection.main;
  const moved = view.moveVertically(range, direction === "down");
  const nextRange = extend
    ? EditorSelection.range(
        range.anchor,
        moved.head,
        moved.goalColumn,
        moved.bidiLevel ?? undefined,
        moved.assoc,
      )
    : EditorSelection.cursor(
        moved.head,
        moved.assoc,
        moved.bidiLevel ?? undefined,
        moved.goalColumn,
      );
  const nextSelection = selection.replaceRange(nextRange);
  if (nextSelection.eq(selection, true)) {
    return false;
  }

  view.dispatch({
    selection: nextSelection,
    scrollIntoView: true,
    userEvent: extend ? "select.keyboard" : "move.keyboard",
  });
  requestAnimationFrame(() => view.focus());
  return true;
}

(window as any).__handleNativeVerticalArrow = (direction: "up" | "down", extend: boolean) => {
  const view = getActiveEditorView();
  if (!view) {
    debugLog(`[arrow] native ${direction} extend=${extend} no active CodeMirror view active=${document.activeElement?.tagName ?? "none"}`);
    return false;
  }

  const before = selectionSummary(view);
  const handled = moveVerticalSelection(view, direction, extend);
  const after = selectionSummary(view);
  debugLog(`[arrow] native ${direction} extend=${extend} handled=${handled} before ${before} after ${after}`);
  return handled;
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
  } else if (e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
    const handled = (window as any).__handleNativeVerticalArrow(e.key === "ArrowUp" ? "up" : "down", true);
    if (handled) {
      e.preventDefault();
      e.stopImmediatePropagation();
    }
  }
}, true); // capture phase

// === Toggle reference panel (called by Rust via eval() for Ctrl+.) ===
(window as any).__toggleReferencePanel = () => {
  window.dispatchEvent(new CustomEvent("toggle-reference-panel"));
};

const app = mount(App, { target: document.getElementById("app")! });

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
