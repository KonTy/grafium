import { invoke } from "@tauri-apps/api/core";
import { tick } from "svelte";
import { parseHex } from "./contrast";

export async function revealStartupWindow(): Promise<void> {
  // Hidden WebKit windows may suspend animation frames. Flush DOM/styles without
  // waiting for requestAnimationFrame, then color the native surfaces before show.
  await tick();
  const color = getComputedStyle(document.documentElement).getPropertyValue("--bg-primary").trim();
  const { r, g, b } = parseHex(color);
  await invoke("reveal_startup_window", { background: [r, g, b] });
}
