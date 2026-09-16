function marker(event: Event): HTMLElement | null {
  const target = event.target;
  const element = target instanceof Element ? target : target instanceof Node ? target.parentElement : null;
  const link = element?.closest<HTMLElement>("[data-reading-note-label]");
  return link && /^grafium-note-[1-9]\d*$/.test(link.dataset.readingNoteLabel ?? "") ? link : null;
}

export function protectReadingNotePointer(event: Event): void {
  if (!marker(event)) return;
  event.preventDefault();
  event.stopPropagation();
}

export function openReadingNoteFromEvent(event: Event, pageId: string, blockId?: string | null): boolean {
  const link = marker(event);
  if (!link || !pageId) return false;
  event.preventDefault();
  event.stopPropagation();
  window.dispatchEvent(new CustomEvent("open-reading-note", {
    detail: { pageId, footnoteLabel: link.dataset.readingNoteLabel, ...(blockId ? { blockId } : {}) },
  }));
  return true;
}
