import { invoke } from "@tauri-apps/api/core";
import { tick } from "svelte";
import { parseHex } from "./contrast";
import { bootPhases, markBoot } from "./bootMarks";

export async function revealStartupWindow(): Promise<void> {
  // Hidden WebKit windows may suspend animation frames. Flush DOM/styles without
  // waiting for requestAnimationFrame, then color the native surfaces before show.
  await tick();
  const color = getComputedStyle(document.documentElement).getPropertyValue("--bg-primary").trim();
  const { r, g, b } = parseHex(color);
  await invoke("reveal_startup_window", { background: [r, g, b] });
}

/// The window starts hidden so its first frame carries the restored theme rather
/// than flashing a default palette. Restoring reads the saved preference and the
/// system appearance, and both have been observed to stall for seconds on a busy
/// machine — leaving no window at all, so the app looks like it failed to start.
/// Reveal once the restore settles or after this grace period, whichever comes
/// first. A healthy restore takes tens of milliseconds, so normal launches are
/// unaffected and still show a fully themed window.
export const REVEAL_GRACE_MS = 1_500;

export async function revealStartupWindowWhenRestored(
  restored: Promise<unknown>,
  graceMs: number = REVEAL_GRACE_MS,
): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const grace = new Promise<void>((resolve) => {
    timer = setTimeout(resolve, graceMs);
  });
  try {
    // Restore failures are reported by the restore itself; they must not also
    // suppress the window.
    await Promise.race([restored.catch(() => undefined), grace]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
  await revealStartupWindow();
}

let contentReported = false;

/// Reveal happens as soon as the theme and layout are restored, which is before
/// any page content exists. Report again once the first page is actually on
/// screen so the recorded startup covers the whole wait rather than stopping at
/// an empty window.
export async function reportStartupContent(): Promise<void> {
  if (contentReported) return;
  contentReported = true;
  await tick();
  markBoot("first page");
  await invoke("startup_content_ready", { phases: bootPhases() });
}

/// Tests reuse one module instance; let them observe the once-only guard.
export function resetStartupContentReportForTests(): void {
  contentReported = false;
}
