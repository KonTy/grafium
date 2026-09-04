/**
 * Pure text-formatting toggle used by the block editor's selection-formatting
 * shortcuts (bold / italic / strikethrough). Kept free of CodeMirror types so
 * it can be unit-tested directly.
 */

export interface WrapResult {
  /** The full document text after applying the toggle. */
  doc: string;
  /** New selection anchor. */
  selStart: number;
  /** New selection head. */
  selEnd: number;
}

/**
 * Toggle `marker` (e.g. `*`, `**`, `~~`) around the selection `[from, to)`.
 *
 *  - No selection: insert the paired markers and place the cursor between them.
 *  - Selection already wrapped (inside or just outside the range): unwrap it.
 *  - Otherwise: wrap the selection, keeping the inner text selected.
 */
export function toggleWrapText(
  doc: string,
  from: number,
  to: number,
  marker: string
): WrapResult {
  const mlen = marker.length;

  // No selection → insert empty pair, cursor in the middle.
  if (from === to) {
    const newDoc = doc.slice(0, from) + marker + marker + doc.slice(from);
    const cursor = from + mlen;
    return { doc: newDoc, selStart: cursor, selEnd: cursor };
  }

  const selected = doc.slice(from, to);
  const mchar = marker.charAt(0);

  // Already wrapped inside the selection → unwrap, but only when the marker
  // run is EXACTLY `marker` long. Otherwise selecting `**bold**` for an italic
  // toggle would strip one `*` per side and corrupt the bold into `*bold*`.
  if (
    selected.length >= 2 * mlen &&
    selected.startsWith(marker) &&
    selected.endsWith(marker) &&
    // the char just inside each run must not extend the marker run
    selected.charAt(mlen) !== mchar &&
    selected.charAt(selected.length - mlen - 1) !== mchar &&
    // the chars just outside the selection must not extend the run either
    doc.charAt(from - 1) !== mchar &&
    doc.charAt(to) !== mchar
  ) {
    const inner = selected.slice(mlen, selected.length - mlen);
    const newDoc = doc.slice(0, from) + inner + doc.slice(to);
    return { doc: newDoc, selStart: from, selEnd: from + inner.length };
  }

  // Markers sit just outside the selection → unwrap them, again only when the
  // surrounding run is exactly `marker` long (so italic doesn't unwrap bold).
  const before = doc.slice(Math.max(0, from - mlen), from);
  const after = doc.slice(to, to + mlen);
  if (
    from >= mlen &&
    before === marker &&
    after === marker &&
    doc.charAt(from - mlen - 1) !== mchar &&
    doc.charAt(to + mlen) !== mchar
  ) {
    const newDoc = doc.slice(0, from - mlen) + selected + doc.slice(to + mlen);
    return { doc: newDoc, selStart: from - mlen, selEnd: to - mlen };
  }

  // Otherwise wrap, keeping the inner text selected.
  const newDoc = doc.slice(0, from) + marker + selected + marker + doc.slice(to);
  return { doc: newDoc, selStart: from + mlen, selEnd: to + mlen };
}
