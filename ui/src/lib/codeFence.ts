/** Optional markdown list marker on the first line of a block. */
const LIST_PREFIX_RE = /^(?:[-*+]\s+|\d+[.)]\s+)/;

/** First non-empty-ish line of a block, without a leading list marker. */
export function firstContentLine(content: string): string {
  const line = content.trimStart().split(/\r?\n/, 1)[0] ?? "";
  return line.replace(LIST_PREFIX_RE, "").trimStart();
}

export function isFenceOpenLine(line: string): boolean {
  const trimmed = line.trimStart();
  return trimmed.startsWith("```") || trimmed.startsWith("~~~");
}

/** True when this block is (or starts as) a fenced code block. */
export function isFencedCodeBlock(content: string): boolean {
  return isFenceOpenLine(firstContentLine(content));
}
