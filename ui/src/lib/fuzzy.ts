// Fuzzy matching for the page list and tree.
//
// A graph of a few hundred pages is already too long to browse by scrolling,
// and exact-prefix filtering is the wrong tool: people remember a page as
// "linux networking" when it is titled `tech/linux/net-config`. Subsequence
// matching finds it; scoring is what keeps the good match at the top rather
// than whichever page happens to sort first.
//
// Deliberately dependency-free and pure, so it is unit-testable and cheap
// enough to run on every keystroke over the whole page set.

/** A scored match. Higher `score` is a better match. */
export interface FuzzyMatch {
  score: number;
  /** Indices in the haystack that matched, for highlighting. */
  positions: number[];
}

const CONSECUTIVE_BONUS = 8;
const WORD_START_BONUS = 12;
const SEPARATOR_START_BONUS = 10;
const LEADING_PENALTY = -1;
const MAX_LEADING_PENALTY = -12;
/// Charged per character skipped between two matches.
///
/// Without it, boundary bonuses make a *scattered* match beat a dense one:
/// every letter of `c-a-t-s` follows a separator and collects the word-start
/// bonus, out-scoring the literal `cats`. Penalising gaps restores the
/// property people expect — the tighter the match, the better.
const GAP_PENALTY = -6;

/**
 * Score `needle` against `haystack` as a case-insensitive subsequence.
 *
 * Returns `null` when the needle isn't a subsequence at all, which lets the
 * caller drop non-matches without a second pass. An empty needle matches
 * everything with a neutral score, so an empty search box shows the full list
 * rather than nothing.
 *
 * Scoring favours, in order: matches at a word or path boundary (typing
 * "ln" should find `tech/linux/networking` ahead of `colonel`), runs of
 * consecutive characters, and matches near the start.
 */
export function fuzzyScore(haystack: string, needle: string): FuzzyMatch | null {
  const trimmed = needle.trim();
  if (trimmed === "") return { score: 0, positions: [] };

  const hay = haystack.toLowerCase();
  const pin = trimmed.toLowerCase();

  let score = 0;
  let hayIndex = 0;
  let lastMatch = -1;
  const positions: number[] = [];

  for (const ch of pin) {
    // Whitespace in the query is a separator, not something to match: "linux
    // net" should behave like two fragments, not require a literal space.
    if (ch === " ") continue;

    const found = hay.indexOf(ch, hayIndex);
    if (found === -1) return null;

    if (found === lastMatch + 1) {
      // Consecutive and "starts a word" are mutually exclusive: a character
      // immediately after the previous match is continuing a run, not
      // beginning a new fragment, so it shouldn't collect both.
      score += CONSECUTIVE_BONUS;
    } else {
      if (lastMatch >= 0) {
        score += (found - lastMatch - 1) * GAP_PENALTY;
      }
      const prev = found > 0 ? hay[found - 1] : "";
      if (found === 0) {
        score += WORD_START_BONUS;
      } else if (prev === "/" || prev === "\\") {
        score += SEPARATOR_START_BONUS;
      } else if (prev === " " || prev === "-" || prev === "_") {
        score += WORD_START_BONUS;
      }
    }

    positions.push(found);
    lastMatch = found;
    hayIndex = found + 1;
  }

  // Prefer a match that starts early, but bounded so a long path isn't
  // punished out of the results entirely.
  score += Math.max(MAX_LEADING_PENALTY, (positions[0] ?? 0) * LEADING_PENALTY);
  // Shorter haystacks are better matches for the same characters.
  score -= Math.floor(haystack.length / 40);

  return { score, positions };
}

/** Whether `haystack` matches at all. */
export function fuzzyMatches(haystack: string, needle: string): boolean {
  return fuzzyScore(haystack, needle) !== null;
}

/**
 * Rank `items` by how well `key(item)` matches `needle`, dropping non-matches.
 *
 * Ties are broken by the key itself so the order is stable across renders —
 * an unstable sort makes a filtered list visibly shuffle as you type.
 */
export function fuzzyRank<T>(
  items: readonly T[],
  needle: string,
  key: (item: T) => string,
): T[] {
  const scored: Array<{ item: T; score: number; label: string }> = [];
  for (const item of items) {
    const label = key(item);
    const match = fuzzyScore(label, needle);
    if (match) scored.push({ item, score: match.score, label });
  }
  scored.sort((a, b) => b.score - a.score || a.label.localeCompare(b.label));
  return scored.map((entry) => entry.item);
}
