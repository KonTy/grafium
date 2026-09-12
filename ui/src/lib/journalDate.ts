/** Local calendar date as YYYY-MM-DD (not UTC). */
export function formatLocalIsoDate(date: Date = new Date()): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function isJournalDateTitle(title: string | undefined | null): boolean {
  return !!title && /^\d{4}-\d{2}-\d{2}$/.test(title);
}

/** Shift a YYYY-MM-DD title by whole local days. */
export function shiftIsoDate(iso: string, days: number): string {
  const [year, month, day] = iso.split("-").map(Number);
  const date = new Date(year, month - 1, day);
  date.setDate(date.getDate() + days);
  return formatLocalIsoDate(date);
}

/** Insert or replace a journal page in title-descending order (newest first). */
export function insertJournalPageByTitleDesc<T extends { id: string; title: string }>(
  pages: T[],
  page: T,
): T[] {
  const existing = pages.findIndex((item) => item.id === page.id || item.title === page.title);
  if (existing >= 0) {
    return pages.map((item, index) => (index === existing ? page : item));
  }
  const index = pages.findIndex((item) => item.title < page.title);
  if (index === -1) return [...pages, page];
  return [...pages.slice(0, index), page, ...pages.slice(index)];
}
