/**
 * Minimal notification store.
 *
 * Intended for failures of actions the user explicitly took, where the UI
 * would otherwise show nothing and leave them believing it worked. Background
 * or cosmetic fetches (version strings, health checks) should stay silent
 * rather than nagging.
 */

export type ToastSeverity = "error" | "info";

export interface Toast {
  id: number;
  message: string;
  severity: ToastSeverity;
}

const DISMISS_AFTER_MS = 6000;

let nextId = 0;

export const toasts = $state<Toast[]>([]);

export function dismissToast(id: number): void {
  const index = toasts.findIndex((t) => t.id === id);
  if (index !== -1) toasts.splice(index, 1);
}

export function showToast(message: string, severity: ToastSeverity = "error"): void {
  const id = nextId++;
  toasts.push({ id, message, severity });
  setTimeout(() => dismissToast(id), DISMISS_AFTER_MS);
}

/** Formats an unknown thrown value into something worth showing a user. */
export function describeError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
