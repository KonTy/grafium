import {
  CALLOUT_KINDS,
  CALLOUT_META,
  calloutInsert,
} from "./callouts";

/**
 * A pure-text slash-menu / template entry: `apply` is inserted verbatim and the
 * cursor lands at `cursorOffset` (defaults to end of the inserted text).
 */
export type SlashCommand = {
  label: string;
  detail: string;
  apply: string;
  cursorOffset?: number;
  action?: string;
};

/** Callout inserters shared by the `/callout …` slash entries. */
export const CALLOUT_SLASH_COMMANDS: SlashCommand[] = CALLOUT_KINDS.map((kind) => {
  const { text, cursorOffset } = calloutInsert(kind);
  return {
    label: `/callout ${kind}`,
    detail: `Insert a ${CALLOUT_META[kind].title} callout`,
    apply: text,
    cursorOffset,
  };
});

/**
 * Formatting inserters (quote, headings, code) plus the callout entries. Sorted
 * after the task/priority entries so TODO/DONE muscle-memory is unaffected.
 */
export const FORMATTING_SLASH_COMMANDS: SlashCommand[] = [
  { label: "/quote", detail: "Insert a blockquote", apply: "> ", cursorOffset: 2 },
  { label: "/heading 1", detail: "Insert a level 1 heading", apply: "# ", cursorOffset: 2 },
  { label: "/heading 2", detail: "Insert a level 2 heading", apply: "## ", cursorOffset: 3 },
  { label: "/heading 3", detail: "Insert a level 3 heading", apply: "### ", cursorOffset: 4 },
  { label: "/code", detail: "Insert a fenced code block", apply: "```\n\n```", cursorOffset: 4 },
  ...CALLOUT_SLASH_COMMANDS,
];

/**
 * Angle-bracket template entries (`< tip`, `< note`, …) — a Logseq-style `<`
 * path to the same callout inserters.
 */
export const ANGLE_TEMPLATE_COMMANDS: SlashCommand[] = CALLOUT_KINDS.map((kind) => {
  const { text, cursorOffset } = calloutInsert(kind);
  return {
    label: `< ${kind}`,
    detail: `Insert a ${CALLOUT_META[kind].title} callout`,
    apply: text,
    cursorOffset,
  };
});

/**
 * Decide whether the `<` template menu should open for the given line text
 * before the cursor, and which entries to show.
 *
 * Guards (mirroring the slash guard) so the menu never hijacks ordinary text:
 *  - the `<` must be at the start of the line or immediately after whitespace
 *    (never right after a word character, e.g. `a<b`);
 *  - the text typed after `<` must be a prefix of a known callout kind, so
 *    `<foo>` or a comparison like `2 < 3` (once a space follows) shows nothing.
 *
 * Returns the matching entries plus the `from` offset (relative to the start of
 * `beforeCursor`) where the `<` begins, or `null` when the menu must not open.
 */
export function angleTemplateMenu(
  beforeCursor: string
): { from: number; options: SlashCommand[] } | null {
  const m = beforeCursor.match(/(?:^|\s)(<[^\s]*)$/);
  if (!m) return null;
  const token = m[1];
  const typed = token.slice(1).toLowerCase();
  const options = ANGLE_TEMPLATE_COMMANDS.filter((cmd) =>
    cmd.label.slice(2).startsWith(typed)
  );
  if (options.length === 0) return null;
  return { from: beforeCursor.length - token.length, options };
}
