/**
 * Minimal notification store.
 *
 * Intended for failures of actions the user explicitly took, where the UI
 * would otherwise show nothing and leave them believing it worked. Background
 * or cosmetic fetches (version strings, health checks) should stay silent
 * rather than nagging.
 */

export type ToastSeverity = "error" | "info" | "success";

/** An optional follow-up the user can take straight from the notification. */
export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: number;
  message: string;
  severity: ToastSeverity;
  action?: ToastAction;
}

const DISMISS_AFTER_MS = 6000;

/**
 * Actionable toasts linger. A notification whose whole point is a button is
 * useless if it disappears before the user has finished reading the sentence
 * in front of it.
 */
const DISMISS_ACTIONABLE_AFTER_MS = 12000;

let nextId = 0;

export const toasts = $state<Toast[]>([]);

export function dismissToast(id: number): void {
  const index = toasts.findIndex((t) => t.id === id);
  if (index !== -1) toasts.splice(index, 1);
}

export function showToast(
  message: string,
  severity: ToastSeverity = "error",
  action?: ToastAction
): void {
  const id = nextId++;
  toasts.push({ id, message, severity, action });
  setTimeout(() => dismissToast(id), action ? DISMISS_ACTIONABLE_AFTER_MS : DISMISS_AFTER_MS);
}

/** Runs a toast's action and dismisses it — the action is a one-shot. */
export function runToastAction(toast: Toast): void {
  toast.action?.run();
  dismissToast(toast.id);
}

/** Formats an unknown thrown value into something worth showing a user. */
export function describeError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
