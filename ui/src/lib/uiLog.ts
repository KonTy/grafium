import { invoke } from "@tauri-apps/api/core";

/**
 * Sends a diagnostic line to the Rust process log.
 *
 * WebKitGTK does not forward `console.log` to stdout, so UI-side debugging
 * from a terminal or log file needs an explicit bridge. Fire-and-forget:
 * a failed log must never break the interaction it is observing.
 */
export function uiLog(message: string): void {
  void invoke("ui_log", { message }).catch(() => {});
}
