// Selection helpers for the Chat transcript. Kept pure and DOM-generic so the
// "does this selection actually touch the transcript?" decision is unit-tested
// rather than entangled with the component's focus bookkeeping.

/**
 * True when a range overlaps a node's content, using boundary-point comparison
 * (supported everywhere, unlike Range.intersectsNode in some engines). Ranges
 * that merely touch at an endpoint don't count as overlapping.
 */
function rangeIntersectsNode(range: Range, node: Node): boolean {
  const doc = node.ownerDocument;
  if (!doc) return false;
  const nodeRange = doc.createRange();
  nodeRange.selectNodeContents(node);
  // No overlap if the range ends at/before the node starts, or starts at/after
  // the node ends. START_TO_END compares range.end vs node.start; END_TO_START
  // compares range.start vs node.end.
  const endsBeforeNodeStarts = range.compareBoundaryPoints(Range.START_TO_END, nodeRange) <= 0;
  const startsAfterNodeEnds = range.compareBoundaryPoints(Range.END_TO_START, nodeRange) >= 0;
  return !endsBeforeNodeStarts && !startsAfterNodeEnds;
}

/**
 * True when `selection` is a real (non-collapsed) selection that overlaps the
 * `transcript` element — even a drag that *began* outside the transcript and
 * ended inside it. This is what tells the composer's blur handler to leave the
 * caret alone instead of stealing focus and collapsing the user's copy.
 */
export function selectionIntersectsTranscript(
  selection: Selection | null,
  transcript: Element | null,
): boolean {
  if (!selection || !transcript || selection.isCollapsed) return false;
  for (let i = 0; i < selection.rangeCount; i++) {
    if (rangeIntersectsNode(selection.getRangeAt(i), transcript)) return true;
  }
  return false;
}
