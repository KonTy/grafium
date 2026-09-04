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
