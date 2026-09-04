// Shared definitions for Logseq-style admonition / callout blocks.
//
// A callout is a single outliner block whose content is:
//   #+BEGIN_TIP
//   …body…
//   #+END_TIP
// The parser keeps this multi-line body as one block; the renderer turns it
// into a styled <div class="callout callout-tip">…</div>.

export type CalloutKind =
  | "tip"
  | "note"
  | "important"
  | "caution"
  | "pinned"
  | "warning";

export const CALLOUT_KINDS: CalloutKind[] = [
  "tip",
  "note",
  "important",
  "caution",
  "pinned",
  "warning",
];

export interface CalloutMeta {
  icon: string;
  title: string;
}

export const CALLOUT_META: Record<CalloutKind, CalloutMeta> = {
  tip: { icon: "💡", title: "Tip" },
  note: { icon: "📝", title: "Note" },
  important: { icon: "❗", title: "Important" },
  caution: { icon: "⚠️", title: "Caution" },
  pinned: { icon: "📌", title: "Pinned" },
  warning: { icon: "🚧", title: "Warning" },
};

export function isCalloutKind(value: string): value is CalloutKind {
  return (CALLOUT_KINDS as string[]).includes(value.toLowerCase());
}

export interface CalloutInsert {
  /** The text to insert into the editor. */
  text: string;
  /** Offset (from the insertion start) to place the cursor — on the blank
   *  body line, ready for typing. */
  cursorOffset: number;
}

/**
 * Build the insert text for a callout of `kind`. Produces:
 *   #+BEGIN_TIP\n\n#+END_TIP
 * with the cursor positioned on the empty middle (body) line.
 */
export function calloutInsert(kind: CalloutKind): CalloutInsert {
  const tag = kind.toUpperCase();
  const begin = `#+BEGIN_${tag}\n`;
  const text = `${begin}\n#+END_${tag}`;
  return { text, cursorOffset: begin.length };
}
