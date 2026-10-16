/** CommonMark ordered markers: up to three spaces and one to nine digits. */
export function orderedListMarker(line: string): { prefix: string; nextPrefix: string | null } | null {
  const match = /^( {0,3})(\d{1,9})([.)])([ \t]+|$)/.exec(line);
  if (!match) return null;
  const [, indent, digits, delimiter, spacing] = match;
  const next = Number(digits) + 1;
  return {
    prefix: match[0],
    nextPrefix: next < 1_000_000_000 ? `${indent}${next}${delimiter}${spacing || " "}` : null,
  };
}
