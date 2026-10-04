import { fuzzyScore } from "./fuzzy";

/**
 * `#tag` autocomplete for the block editor.
 *
 * Tag syntax mirrors `TAG_RE` in core/src/parser/links.rs: a `#` followed by a
 * letter or number, then letters, numbers, marks, `_`, `/`, `\` or `-`.
 * Anything else (spaces, punctuation) needs the bracketed form `#[[…]]`.
 */
const TAG_BODY = /^[\p{L}\p{N}][\p{L}\p{N}\p{M}_/\\-]*$/u;
/** A `#` at the start of the line or after whitespace/`(`, then tag text. */
const TAG_TOKEN = /(?:^|[\s(])#([\p{L}\p{N}\p{M}_/\\-]*)$/u;
const TAG_SUFFIX = /^[\p{L}\p{N}\p{M}_/\\-]*/u;

/** How many suggestions the `#` picker shows. */
export const TAG_RESULT_LIMIT = 20;

export type TagToken = {
  /** Index of the `#` in the line text before the cursor. */
  from: number;
  /** Tag text typed so far (may be empty right after `#`). */
  query: string;
};

export type TagCandidate = {
  title: string;
  /** Newest edit, for ordering when nothing is typed yet. */
  updated_at?: number;
  is_journal?: boolean;
};

export type TagSuggestion = {
  title: string;
  /**
   * An existing tag, a page that has never been used as one, or what was
   * typed, offered as a new tag.
   */
  kind: "tag" | "page" | "new";
};

/**
 * Detect a `#tag` being typed immediately before the cursor.
 *
 * A heading (`# Title`) never matches: the space ends the token. Neither do
 * `##`, URL fragments (`page#part`) or text inside inline code.
 */
export function tagToken(beforeCursor: string): TagToken | null {
  const match = TAG_TOKEN.exec(beforeCursor);
  if (!match) return null;
  const query = match[1];
  if (query && !/^[\p{L}\p{N}]/u.test(query)) return null;
  const from = beforeCursor.length - query.length - 1;
  const backticks = beforeCursor.slice(0, from).match(/`/g)?.length ?? 0;
  if (backticks % 2 === 1) return null;
  return { from, query };
}

/** The text inserted for a chosen tag: `#tag`, or `#[[Two words]]`. */
export function tagInsertText(title: string): string {
  return TAG_BODY.test(title) ? `#${title}` : `#[[${title}]]`;
}

/** Tag characters already after the cursor, replaced when completing mid-tag. */
export function tagSuffixLength(afterCursor: string): number {
  return TAG_SUFFIX.exec(afterCursor)?.[0].length ?? 0;
}

const EXACT = 0;
const LOOSE = 4;

function matchClass(title: string, query: string): number | null {
  const lower = title.toLowerCase();
  if (lower === query) return EXACT;
  if (lower.startsWith(query)) return 1;
  const at = lower.indexOf(query);
  if (at > 0) return /[\s/_\-\\]/.test(lower[at - 1]) ? 2 : 3;
  return fuzzyScore(title, query) ? LOOSE : null;
}

/**
 * With nothing typed: existing tags, newest first, then other pages.
 *
 * With text typed, Enter takes the first choice, so it must never turn what
 * was typed into a different tag:
 * - a title matching exactly, tag or page, comes first;
 * - then titles containing the text as written (tags, then pages; at the
 *   start, then at a word start, then anywhere);
 * - then, unless a title matched exactly, the text itself as a new tag;
 * - then loose fuzzy matches.
 *
 * Nothing is suggested when no title matches at all, so Enter keeps its
 * usual meaning.
 */
export function rankTagSuggestions(
  tags: readonly TagCandidate[],
  pages: readonly TagCandidate[],
  query: string,
  limit = TAG_RESULT_LIMIT,
): TagSuggestion[] {
  const typed = query.trim();
  const needle = typed.toLowerCase();
  const seen = new Set<string>();
  const ranked: Array<{ title: string; isTag: boolean; match: number; rank: number[] }> = [];
  const add = (candidate: TagCandidate, isTag: boolean) => {
    const key = candidate.title.toLowerCase();
    if (!candidate.title || seen.has(key)) return;
    if (!isTag && candidate.is_journal) return;
    if (needle) {
      const match = matchClass(candidate.title, needle);
      if (match === null) return;
      const score = fuzzyScore(candidate.title, needle)?.score ?? 0;
      ranked.push({
        title: candidate.title, isTag, match,
        rank: [match === EXACT ? 0 : 1, match === LOOSE ? 1 : 0, isTag ? 0 : 1, match, -score, candidate.title.length],
      });
    } else {
      ranked.push({ title: candidate.title, isTag, match: EXACT, rank: [isTag ? 0 : 1, -(candidate.updated_at ?? 0)] });
    }
    seen.add(key);
  };
  for (const tag of tags) add(tag, true);
  for (const page of pages) add(page, false);
  ranked.sort((a, b) => {
    for (let index = 0; index < Math.max(a.rank.length, b.rank.length); index += 1) {
      const delta = (a.rank[index] ?? 0) - (b.rank[index] ?? 0);
      if (delta) return delta;
    }
    return a.title.localeCompare(b.title);
  });
  const suggest = ({ title, isTag }: { title: string; isTag: boolean }): TagSuggestion =>
    ({ title, kind: isTag ? "tag" : "page" });
  if (!needle || ranked.length === 0 || ranked[0].match === EXACT) return ranked.slice(0, limit).map(suggest);
  const room = Math.max(0, limit - 1);
  const contained = ranked.filter(({ match }) => match !== LOOSE).slice(0, room);
  const loose = ranked.filter(({ match }) => match === LOOSE).slice(0, room - contained.length);
  return [...contained.map(suggest), { title: typed, kind: "new" }, ...loose.map(suggest)];
}

const TAG_CACHE_MS = 30_000;
let tagCache: { at: number; tags: Promise<TagCandidate[]> } | null = null;

/** Forget cached tags, e.g. after the page tree changed. */
export function invalidateTagCandidates(): void {
  tagCache = null;
}

if (typeof window !== "undefined") {
  // Fired whenever pages or tags are added, renamed or removed.
  window.addEventListener("page-tree-refresh", invalidateTagCandidates);
}

/**
 * Suggestions for a typed `#query`. Tags are fetched once and filtered here,
 * so narrowing the query never waits on the database; other pages come from
 * the title search.
 */
export async function loadTagSuggestions(
  query: string,
  fetchers: {
    listTags: () => Promise<TagCandidate[]>;
    searchPages: (query: string, limit: number) => Promise<TagCandidate[]>;
    listRecent: (limit: number) => Promise<TagCandidate[]>;
  },
  now = Date.now(),
): Promise<TagSuggestion[]> {
  if (!tagCache || now - tagCache.at > TAG_CACHE_MS) {
    const tags = fetchers.listTags();
    tagCache = { at: now, tags };
    tags.catch(() => {
      if (tagCache?.tags === tags) tagCache = null;
    });
  }
  const trimmed = query.trim();
  const [tags, pages] = await Promise.all([
    tagCache.tags,
    trimmed ? fetchers.searchPages(trimmed, 40) : fetchers.listRecent(TAG_RESULT_LIMIT),
  ]);
  return rankTagSuggestions(tags, pages, trimmed);
}
