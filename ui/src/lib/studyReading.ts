import type { StudyProgress } from "./studies";
import { showToast } from "./toast.svelte";

const BLOCK_SELECTOR = "[data-block-id], [data-source-block-id]";
function visibleTop(main: HTMLElement): number {
  return main.getBoundingClientRect().top
    + (main.querySelector(".study-session-controls")?.getBoundingClientRect().height ?? 0);
}
export function readingProgress(main: HTMLElement): StudyProgress {
  const top = visibleTop(main);
  const block = [...main.querySelectorAll<HTMLElement>(BLOCK_SELECTOR)]
    .find(element => element.getBoundingClientRect().bottom > top + 20);
  const id = block?.dataset.blockId ?? block?.dataset.sourceBlockId ?? "";
  const total = Math.max(0, main.scrollHeight - main.clientHeight);
  const position = Math.max(0, Math.min(total, main.scrollTop));
  return {
    position, total,
    anchor: id ? JSON.stringify({ id, offset: Math.max(0, top - block!.getBoundingClientRect().top) }) : "",
    label: total > 0 ? `${Math.round(position / total * 100)}% read (scroll position)` : "Single screen",
  };
}

export function restoreReadingProgress(main: HTMLElement, progress: StudyProgress): boolean {
  if (progress.anchor) {
    try {
      const anchor: unknown = JSON.parse(progress.anchor);
      if (anchor && typeof anchor === "object" && "id" in anchor && typeof anchor.id === "string"
        && "offset" in anchor && typeof anchor.offset === "number" && Number.isFinite(anchor.offset)) {
        const id = CSS.escape(anchor.id);
        const block = main.querySelector<HTMLElement>(`[data-block-id="${id}"], [data-source-block-id="${id}"]`);
        if (block) {
          main.scrollTop += block.getBoundingClientRect().top - visibleTop(main) + anchor.offset;
          return true;
        }
      }
    } catch (error) {
      showToast(`The saved reading anchor is invalid; using scroll position instead. ${String(error)}`, "error");
    }
  }
  const total = Math.max(0, main.scrollHeight - main.clientHeight);
  if (total > 0) main.scrollTop = progress.total > 0 ? total * progress.position / progress.total : 0;
  return !progress.anchor && total > 0;
}

export function trackReading(
  main: HTMLElement, saved: StudyProgress,
  onProgress: (progress: StudyProgress) => void, onActivity: () => void,
): () => void {
  let restoring = true;
  let disposed = false;
  let frame = 0;
  const restore = () => {
    if (disposed || !restoring) return;
    if (restoreReadingProgress(main, saved)) finishRestore();
  };
  const observer = new MutationObserver(() => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(restore);
  });
  const finishRestore = () => {
    if (!restoring || disposed) return;
    restoring = false;
    observer.disconnect();
    onActivity();
  };
  observer.observe(main, { childList: true, subtree: true });
  frame = requestAnimationFrame(restore);
  const timeout = setTimeout(finishRestore, 2500);
  const interacted = () => { if (restoring) finishRestore(); };
  const scrolled = () => {
    if (restoring) return;
    onActivity();
    onProgress(readingProgress(main));
  };
  main.addEventListener("wheel", interacted, { passive: true });
  main.addEventListener("touchstart", interacted, { passive: true });
  main.addEventListener("keydown", interacted);
  main.addEventListener("scroll", scrolled, { passive: true });
  return () => {
    disposed = true;
    clearTimeout(timeout);
    cancelAnimationFrame(frame);
    observer.disconnect();
    main.removeEventListener("wheel", interacted);
    main.removeEventListener("touchstart", interacted);
    main.removeEventListener("keydown", interacted);
    main.removeEventListener("scroll", scrolled);
  };
}
