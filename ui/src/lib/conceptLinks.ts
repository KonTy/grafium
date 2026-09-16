/** Display-only concept label. Never use this slug to rename or resolve a page. */
export function formatConceptTag(title: string): string {
  const slug = title.toLowerCase().replace(/[^\p{L}\p{N}\p{M}]+/gu, "_").replace(/^_+|_+$/g, "");
  return /^[\p{L}\p{N}]/u.test(slug) ? `#${slug}` : "";
}

export function formatConceptLink(title: string): string | null {
  const target = title.trim();
  const display = formatConceptTag(target);
  if (!display || /[\[\]|\p{Cc}]/u.test(target)
    || /\b(?:[a-z][a-z0-9+.-]*:\/\/|mailto:|www\.)/i.test(target)
    || /\(\([^)\r\n]+\)\)/.test(target)) return null;
  return `[[${target}|${display}]]`;
}
