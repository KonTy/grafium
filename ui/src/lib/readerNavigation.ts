export type ReaderTurn = "prev" | "next" | "left" | "right";
export type ReaderFlow = "paginated" | "scrolled";

export function readerNavigationKey(event: KeyboardEvent, doc: Document): ReaderTurn | null {
  if (event.defaultPrevented || event.isComposing || event.ctrlKey || event.metaKey
    || event.altKey || event.shiftKey || doc.getSelection()?.toString()) return null;
  const target = event.target as Element | null;
  if (target?.closest?.("input,textarea,select,[contenteditable],[role=textbox],[role=slider],[role=menu]")) return null;
  switch (event.key) {
    case "ArrowLeft": return "left";
    case "ArrowRight": return "right";
    case "PageUp": return "prev";
    case "PageDown": return "next";
    default: return null;
  }
}

export function readerOwnsNavigation(event: KeyboardEvent, root: Element): boolean {
  const doc = root.ownerDocument;
  return !!readerNavigationKey(event, doc) && (event.target === doc.body
    || event.target === doc.documentElement || event.target === doc
    || event.target instanceof Node && root.contains(event.target));
}
