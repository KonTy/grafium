/**
 * Conflict markers that sync writes into a note when two devices changed the
 * same spot (see `core/src/sync/merge.rs`). On disk and in the outliner they
 * are ordinary lines, one block each; this module only recognises them so the
 * editor can make them stand out. The format matches the Rust scanner, which
 * keeps a note out of sync while any marker is left in it.
 */

export type MergeMarkerKind = "yours" | "divider" | "theirs";

const MARKER_RE = /^(<<<<<<< YOURS|=======|>>>>>>> THEIRS) \(merge [0-9a-f]{8}\)$/;

const KIND_BY_LABEL: Record<string, MergeMarkerKind> = {
  "<<<<<<< YOURS": "yours",
  "=======": "divider",
  ">>>>>>> THEIRS": "theirs",
};

const HINT_BY_KIND: Record<MergeMarkerKind, string> = {
  yours: "Your version, from this device. Keep what you want, then delete the marker lines.",
  divider: "The other device's version follows.",
  theirs: "End of the other device's version.",
};

/** Which marker a block's text is, or `null` for anything else. */
export function mergeMarkerKind(content: string): MergeMarkerKind | null {
  const match = MARKER_RE.exec(content.trim());
  return match ? KIND_BY_LABEL[match[1]] : null;
}

/** HTML for a marker block, or `null` when the block is not a marker. */
export function renderMergeMarkerBlock(content: string): string | null {
  const kind = mergeMarkerKind(content);
  if (kind === null) return null;
  // Only fixed labels and an 8-digit hex token can reach here, so the text
  // needs escaping for `<` and `>` and nothing else.
  const text = content.trim().replace(/</g, "&lt;").replace(/>/g, "&gt;");
  return `<div class="merge-marker merge-marker-${kind}" title="${HINT_BY_KIND[kind]}">${text}</div>`;
}
