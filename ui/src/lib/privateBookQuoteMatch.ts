import type { BookLocation } from "./bookLocations";

export interface QuoteSegment { text: string; locator: Extract<BookLocation, { kind: "epub" }> }

function decodeEntities(value: string): string {
  const named: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };
  return value.replace(/&(#x[\da-f]+|#\d+|[a-z]+);/gi, (match, entity: string) => {
    if (entity[0] === "#") {
      const code = entity[1]?.toLowerCase() === "x" ? Number.parseInt(entity.slice(2), 16) : Number.parseInt(entity.slice(1), 10);
      return Number.isFinite(code) ? String.fromCodePoint(code) : match;
    }
    return named[entity.toLowerCase()] ?? match;
  });
}

export function normalizeLibraryQuote(value: string): string {
  return decodeEntities(value)
    .replace(/<sup\b[^>]*>.*?<\/sup>/gis, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/__([^_]+)__/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/[*_~#>\[\]()`]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .toLocaleLowerCase();
}

function compact(value: string): string { return value.replace(/\s+/g, ""); }

function quoteNeedles(quote: string): string[] {
  const normalized = compact(normalizeLibraryQuote(quote));
  if (normalized.length < 40) return [];
  const sizes = [80, 70, 60, 50, 40].filter(size => normalized.length >= size);
  const needles = sizes.map(size => normalized.slice(0, size).trim()).filter(value => value.length >= 40);
  if (!needles.length && normalized.length >= 40) needles.push(normalized.slice(0, 40).trim());
  return [...new Set(needles)];
}

function joinedSection(segments: QuoteSegment[]): { text: string; map: number[]; normalizedSegments: string[] } {
  let text = "";
  const map: number[] = [];
  const normalizedSegments: string[] = [];
  for (const [index, segment] of segments.entries()) {
    const normalized = normalizeLibraryQuote(segment.text);
    normalizedSegments.push(compact(normalized));
    for (const char of normalized) {
      if (/\s/.test(char)) continue;
      text += char; map.push(index);
    }
  }
  return { text, map, normalizedSegments };
}

export function findQuoteSegment(quote: string | null | undefined, sections: QuoteSegment[][]): QuoteSegment | null {
  const needles = quoteNeedles(quote ?? "");
  if (!needles.length) return null;
  const quoteStart = needles[needles.length - 1];
  for (const section of sections) {
    const joined = joinedSection(section);
    for (const needle of needles) {
      const found = joined.text.indexOf(needle);
      if (found >= 0) return section[joined.map[Math.min(found, joined.map.length - 1)]] ?? null;
    }
    const reverseIndex = joined.normalizedSegments.findIndex(segment => segment.length >= 40 && quoteStart.startsWith(segment.slice(0, Math.min(segment.length, quoteStart.length))));
    if (reverseIndex >= 0) return section[reverseIndex] ?? null;
  }
  return null;
}
