import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export interface ReaderFullscreen {
  exit(): Promise<void>;
  isActive(): Promise<boolean>;
}

export async function enterReaderFullscreen(element: HTMLElement): Promise<ReaderFullscreen> {
  if (isTauri()) {
    const window = getCurrentWindow();
    const wasFullscreen = await window.isFullscreen();
    if (!wasFullscreen) await window.setFullscreen(true);
    return {
      exit: async () => { if (!wasFullscreen) await window.setFullscreen(false); },
      isActive: () => window.isFullscreen(),
    };
  }
  if (!element.requestFullscreen) throw new Error("Fullscreen is not supported by this browser.");
  await element.requestFullscreen();
  return {
    exit: async () => { if (document.fullscreenElement === element) await document.exitFullscreen(); },
    isActive: async () => document.fullscreenElement === element,
  };
}
