import { mergeMarkerKind } from "./mergeMarkers";

export function getHeadingLevel(content: string): number {
  const match = content.trimStart().match(/^(#{1,6})\s+/);
  return match ? match[1].length : 0;
}

export function getBulletMinHeight(content: string): string {
  switch (getHeadingLevel(content)) {
    case 1:
      return "32px";
    case 2:
      return "28px";
    case 3:
      return "25px";
    default:
      return "24px";
  }
}

/**
 * Whether a block is a quotation. A sync conflict marker line ends in
 * `>>>>>>> THEIRS`, which starts with `>` but quotes nobody.
 */
export function isQuoteBlockContent(content: string): boolean {
  return content.trimStart().startsWith(">") && mergeMarkerKind(content) === null;
}
