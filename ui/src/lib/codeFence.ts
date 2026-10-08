/** Optional markdown list marker on the first line of a block. */
const LIST_PREFIX_RE = /^(?:[-*+]\s+|\d+[.)]\s+)/;

/** First non-empty-ish line of a block, without a leading list marker. */
export function firstContentLine(content: string): string {
  const line = content.trimStart().split(/\r?\n/, 1)[0] ?? "";
  return line.replace(LIST_PREFIX_RE, "").trimStart();
}

export function isFenceOpenLine(line: string): boolean {
  return openingCodeFence(line) !== null;
}

export interface CodeFence {
  char: "`" | "~";
  length: number;
}

export function openingCodeFence(line: string): CodeFence | null {
  const match = /^[ \t]*(`{3,}|~{3,})(.*)$/.exec(line.replace(/\r$/, ""));
  if (!match || (match[1][0] === "`" && match[2].includes("`"))) return null;
  return { char: match[1][0] === "`" ? "`" : "~", length: match[1].length };
}

export function closesCodeFence(line: string, fence: CodeFence): boolean {
  const match = /^[ \t]*(`{3,}|~{3,})[ \t]*\r?$/.exec(line);
  return !!match && match[1][0] === fence.char && match[1].length >= fence.length;
}

export function nextCodeFence(line: string, active: CodeFence | null): CodeFence | null {
  return active ? (closesCodeFence(line, active) ? null : active) : openingCodeFence(line);
}

/** Includes the opening line's end and closing line, where Enter must not split the block. */
export function isInsideCodeFenceAt(content: string, position: number): boolean {
  let active: CodeFence | null = null;
  let from = 0;
  for (const line of content.split("\n")) {
    const to = from + line.length;
    const logicalLine = active ? line : firstContentLine(line);
    if (position <= to) return active !== null || (position === to && openingCodeFence(logicalLine) !== null);
    active = nextCodeFence(logicalLine, active);
    from = to + 1;
  }
  return active !== null;
}

/** True when this block is (or starts as) a fenced code block. */
export function isFencedCodeBlock(content: string): boolean {
  return isFenceOpenLine(firstContentLine(content));
}
