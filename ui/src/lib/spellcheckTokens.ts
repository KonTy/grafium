import { shouldRenderInlineMath } from "./markdown";

/**
 * Finds the words in a note that are worth spell checking.
 *
 * Underlining code, links, tags and identifiers would be worse than no spell
 * checker, so this errs on the side of leaving things alone: only runs of
 * letters (with an inner apostrophe or hyphen) are words, and anything that
 * looks technical is skipped.
 */

export type SpellWord = { from: number; to: number; word: string };

/** Longest word sent for checking; the Rust side ignores longer ones anyway. */
export const MAX_SPELL_WORD_CHARS = 64;

export type TextRange = readonly [from: number, to: number];

/**
 * Syntax that is not prose. Mirrors the renderer (`lib/markdown.ts`) and the
 * page parser (`core/src/parser`); code is left to the editor's syntax tree.
 */
const NON_PROSE: RegExp[] = [
  /\[\[[^[\]\r\n]+\]\]/g, // [[page links]]
  /\(\([^)\n]+\)\)/g, // ((block refs))
  /\{\{[^}\n]*\}\}/g, // {{macros}}
  /#[\p{L}\p{N}][\p{L}\p{N}\p{M}_/\\-]*/gu, // #tags
  /\[#[ABC]\]/gi, // [#A] priorities
  /:icon-[a-z0-9-]+:/g, // :icon-name:
  /\[\^[^\]\s]+\]/g, // [^footnotes]
  /^[ \t]*(?:[-*+][ \t]+)?[a-zA-Z_-]+::.*$/gm, // key:: value properties
  /`[^`\n]+`/g, // `inline code`, for text the syntax tree has not reached
  /<!--[\s\S]*?-->/g, // HTML comments
  /<\/?[A-Za-z][^<>\n]*>/g, // HTML tags
  /(?:[A-Za-z][A-Za-z0-9+.-]*:\/\/|www\.)[^\s<>"]+/g, // URLs
  /[\p{L}\p{N}._%+-]+@[\p{L}\p{N}-]+(?:\.[\p{L}\p{N}-]+)+/gu, // e-mail addresses
  /\$\$[\s\S]+?(?<!\\)\$\$/g, // $$display math$$
];

const INLINE_MATH = /\$([^\n$]+?)(?<!\\)\$/g;

/** Ranges of `text` that hold the app's own syntax rather than prose. */
export function nonProseRanges(text: string): TextRange[] {
  const ranges: TextRange[] = [];
  for (const pattern of NON_PROSE) {
    for (const match of text.matchAll(pattern)) {
      ranges.push([match.index, match.index + match[0].length]);
    }
  }
  for (const match of text.matchAll(INLINE_MATH)) {
    // `$5 and $10` is prose, which the renderer shows as plain text.
    if (shouldRenderInlineMath(match[1])) {
      ranges.push([match.index, match.index + match[0].length]);
    }
  }
  return ranges;
}

/** `text` with every character in `ranges` replaced by a space. */
function blankOut(text: string, ranges: readonly TextRange[]): string {
  if (ranges.length === 0) return text;
  const sorted = [...ranges].sort((a, b) => a[0] - b[0]);
  let out = "";
  let at = 0;
  for (const [from, to] of sorted) {
    const start = Math.max(from, at);
    const end = Math.min(to, text.length);
    if (end <= start) continue;
    out += text.slice(at, start) + " ".repeat(end - start);
    at = end;
  }
  return out + text.slice(at);
}

// Marks that end a word without making its neighbour technical.
const RUN = /[^\s\u200b,;!?(){}[\]|"“”„‟«»‹›…—–¿¡।॥،؛؟]+/gu;
// Punctuation that may wrap a word: `*emphasis*`, `'quoted'`, `end.`, `Note:`.
const WRAPPER = "[.:*_~'‘’ʼ\\-‐‑]";
const LEADING = new RegExp(`^${WRAPPER}+`);
const TRAILING = new RegExp(`${WRAPPER}+$`);
const LETTER = "[\\p{L}\\p{M}\\u00ad\\u200c\\u200d]";
const WORD_SHAPE = new RegExp(`^${LETTER}+(?:['’ʼ\\-‐‑]${LETTER}+)*$`, "u");

/** Acronyms, `camelCase` and `McNames`: not worth checking. */
function isNameLike(word: string): boolean {
  if (word === word.toUpperCase() && word !== word.toLowerCase()) return true;
  const [first] = word;
  return /\p{Lu}/u.test(word.slice(first.length));
}

/**
 * The words to check in `text`.
 *
 * `skip` lists ranges of `text` to leave out (such as code); positions in the
 * result are `offset` plus the position in `text`.
 */
export function findSpellWords(
  text: string,
  skip: readonly TextRange[] = [],
  offset = 0,
): SpellWord[] {
  const masked = blankOut(text, [...nonProseRanges(text), ...skip]);
  const words: SpellWord[] = [];
  for (const match of masked.matchAll(RUN)) {
    const run = match[0];
    const start = LEADING.exec(run)?.[0].length ?? 0;
    const end = run.length - (TRAILING.exec(run)?.[0].length ?? 0);
    if (end <= start) continue;
    const word = run.slice(start, end);
    if (word.length > MAX_SPELL_WORD_CHARS || !WORD_SHAPE.test(word)) continue;
    if ([...word].length < 2 || isNameLike(word)) continue;
    const from = offset + match.index + start;
    words.push({ from, to: from + word.length, word });
  }
  return words;
}
