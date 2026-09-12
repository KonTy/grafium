import { fuzzyRank } from "./fuzzy";

/** How many titles to pull from the DB before client-side ranking. */
export const WIKI_LINK_SEARCH_LIMIT = 40;
/** How many rows the `[[` picker shows. */
export const WIKI_LINK_RESULT_LIMIT = 20;

export type WikiLinkPage = {
  title: string;
  is_journal?: boolean;
};

export type WikiLinkToken = {
  /** Index of `[[` in the line text before the cursor. */
  from: number;
  /** Text between `[[` and the cursor (may be empty). */
  query: string;
};

/**
 * Detect an unclosed wiki-link token immediately before the cursor.
 *
 * Stops at a closed `]]` or a newline so a finished link, or a `[` that is
 * not a wiki opener, never hijacks the slash/`<` menus.
 */
export function wikiLinkToken(beforeCursor: string): WikiLinkToken | null {
  const start = beforeCursor.lastIndexOf("[[");
  if (start < 0) return null;
  const inner = beforeCursor.slice(start + 2);
  if (inner.includes("]]") || inner.includes("\n")) return null;
  return { from: start, query: inner };
}

export function wikiLinkReplacement(title: string): string {
  return `[[${title}]]`;
}

/**
 * Extra characters after the cursor that belong to a half-typed closer,
 * so picking a title replaces `[[query]]` instead of producing `[[Title]]]]`.
 */
export function wikiLinkCloseExtra(afterCursor: string): number {
  if (afterCursor.startsWith("]]")) return 2;
  if (afterCursor.startsWith("]")) return 1;
  return 0;
}

export function rankWikiLinkTitles<T extends WikiLinkPage>(
  pages: readonly T[],
  query: string,
  limit = WIKI_LINK_RESULT_LIMIT,
): T[] {
  if (!query.trim()) return pages.slice(0, limit);
  return fuzzyRank(pages, query, (page) => page.title).slice(0, limit);
}

export async function loadWikiLinkPages(
  query: string,
  fetchers: {
    search: (query: string, limit: number) => Promise<WikiLinkPage[]>;
    listRecent: (limit: number) => Promise<WikiLinkPage[]>;
  },
): Promise<WikiLinkPage[]> {
  const trimmed = query.trim();
  const pages = trimmed
    ? await fetchers.search(trimmed, WIKI_LINK_SEARCH_LIMIT)
    : await fetchers.listRecent(WIKI_LINK_RESULT_LIMIT);
  return rankWikiLinkTitles(pages, query);
}
