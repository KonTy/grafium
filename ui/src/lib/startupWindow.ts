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
