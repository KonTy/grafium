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
 * Alias text is not a title query. Stop after `|`, a closing bracket, or a
 * newline so editing a display label never opens the page picker.
 */
export function wikiLinkToken(beforeCursor: string): WikiLinkToken | null {
  const start = beforeCursor.lastIndexOf("[[");
  if (start < 0) return null;
  const escapes = /\\+$/.exec(beforeCursor.slice(0, start))?.[0].length ?? 0;
  if (escapes % 2 === 1) return null;
  const inner = beforeCursor.slice(start + 2);
  if (/[\[\]|\r\n]/.test(inner)) return null;
  return { from: start, query: inner };
}

function wikiLinkSuffix(afterCursor: string): { length: number; alias?: string } {
  const closed = /^([^\[\]\r\n]*)(\]\]?)/.exec(afterCursor);
  const unclosedAlias = /^\|[^\[\]\r\n]*$/.exec(afterCursor);
  const inner = closed?.[1] ?? unclosedAlias?.[0];
  if (inner === undefined) return { length: 0 };
  const separator = inner.indexOf("|");
  return {
    length: closed?.[0].length ?? inner.length,
    alias: separator < 0 ? undefined : inner.slice(separator + 1),
  };
}

export function wikiLinkReplacement(title: string, afterCursor = ""): string {
  const { alias } = wikiLinkSuffix(afterCursor);
  return alias === undefined ? `[[${title}]]` : `[[${title}|${alias}]]`;
}

/**
 * Consume the existing target suffix, alias, and closer when completing from
 * anywhere in a title. Pass the same suffix to `wikiLinkReplacement`.
 */
export function wikiLinkCloseExtra(afterCursor: string): number {
  return wikiLinkSuffix(afterCursor).length;
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
