export interface CurrentBlockAnchor {
  pageId: string;
  blockId: string | null;
}

let currentBlockAnchor: CurrentBlockAnchor | null = null;

export function setCurrentBlockAnchor(pageId: string, blockId: string | null) {
  if (blockId !== null || currentBlockAnchor?.pageId === pageId) {
    currentBlockAnchor = { pageId, blockId };
  }
  if (typeof window === "undefined") return;
  window.dispatchEvent(
    new CustomEvent("page-content-focus-changed", { detail: { pageId, blockId } }),
  );
}

export function getCurrentBlockAnchor(pageId: string): string | null {
  if (!currentBlockAnchor || currentBlockAnchor.pageId !== pageId) return null;
  return currentBlockAnchor.blockId;
}

export function getLatestCurrentBlockAnchor(): CurrentBlockAnchor | null {
  if (!currentBlockAnchor?.blockId) return null;
  return currentBlockAnchor;
}
