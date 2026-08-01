import { performRedo, performUndo } from "./undoStack";

export interface AppUndoEventTarget {
  addEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject | null,
    options?: boolean | AddEventListenerOptions
  ): void;
  removeEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject | null,
    options?: boolean | EventListenerOptions
  ): void;
}

export function attachAppUndoRedoListeners(target: AppUndoEventTarget = window): () => void {
  const handleUndo: EventListener = () => {
    void performUndo();
  };
  const handleRedo: EventListener = () => {
    void performRedo();
  };

  target.addEventListener("app-undo", handleUndo);
  target.addEventListener("app-redo", handleRedo);

  return () => {
    target.removeEventListener("app-undo", handleUndo);
    target.removeEventListener("app-redo", handleRedo);
  };
}
