export interface SourceLine {
  number: number;
  index: number;
  from: number;
  to: number;
  text: string;
}

export interface SourceRange {
  from: number;
  to: number;
}

export interface SourceContentSegment extends SourceRange {
  line: SourceLine;
  contentFrom: number;
  contentTo: number;
  text: string;
}

export interface SourcePropertyLine extends SourceRange {
  line: SourceLine;
  key: string;
  value: string;
}

export interface SourceBlock {
  id: string | null;
  depth: number;
  indent: string;
  line: SourceLine;
  ownFrom: number;
  ownTo: number;
  subtreeFrom: number;
  subtreeTo: number;
  contentFrom: number;
  contentTo: number;
  previewFrom: number;
  previewTo: number;
  content: string;
  contentSegments: SourceContentSegment[];
  propertyLines: SourcePropertyLine[];
  idLine: SourcePropertyLine | null;
}

export interface PageSourceMap {
  source: string;
  lines: SourceLine[];
  blocks: SourceBlock[];
  idLines: SourcePropertyLine[];
}

export interface SourceReplacement {
  from: number;
  to: number;
  insert: string;
}

interface BlockStart {
  lineIndex: number;
  depth: number;
  indent: string;
  contentColumn: number;
  content: string;
}

const BLOCK_START_RE = /^([ \t]*)-(?:[ \t](.*)|$)/;
const PROPERTY_RE = /^([a-zA-Z_-]+)::(.*)$/;

export function splitSourceLines(source: string): SourceLine[] {
  const parts = source.split("\n");
  const lines: SourceLine[] = [];
  let from = 0;

  for (let index = 0; index < parts.length; index += 1) {
    const text = parts[index] ?? "";
    const to = from + text.length;
    lines.push({
      number: index + 1,
      index,
      from,
      to,
      text,
    });
    from = to + 1;
  }

  return lines;
}

export function countSourceIndent(line: string): number {
  let depth = 0;
  let spaces = 0;

  for (const char of line) {
    if (char === "\t") {
      depth += 1;
      spaces = 0;
    } else if (char === " ") {
      spaces += 1;
      if (spaces === 2) {
        depth += 1;
        spaces = 0;
      }
    } else {
      break;
    }
  }

  return depth;
}

export function matchSourceBlockStart(line: string): Omit<BlockStart, "lineIndex" | "depth"> | null {
  const match = BLOCK_START_RE.exec(line);
  if (!match) return null;

  const indent = match[1] ?? "";
  const content = match[2] ?? "";
  return {
    indent,
    contentColumn: line.length - content.length,
    content,
  };
}

function stripContinuation(line: string, minDepth: number): { column: number; text: string } {
  let column = 0;
  let depth = 0;
  let spaces = 0;

  while (column < line.length && depth < minDepth) {
    const char = line[column];
    if (char === "\t") {
      column += 1;
      depth += 1;
      spaces = 0;
    } else if (char === " ") {
      column += 1;
      spaces += 1;
      if (spaces === 2) {
        depth += 1;
        spaces = 0;
      }
    } else {
      break;
    }
  }

  return { column, text: line.slice(column) };
}

function matchProperty(line: string): { key: string; value: string } | null {
  const match = PROPERTY_RE.exec(line.trimStart());
  if (!match) return null;

  return {
    key: match[1],
    value: (match[2] ?? "").trim(),
  };
}

function isFenceMarker(text: string): boolean {
  return text.trimStart().startsWith("```");
}

function rangeIntersects(a: SourceRange, b: SourceRange): boolean {
  return a.from <= b.to && b.from <= a.to;
}

export function sourceRangesIntersect(aFrom: number, aTo: number, bFrom: number, bTo: number): boolean {
  return rangeIntersects({ from: aFrom, to: aTo }, { from: bFrom, to: bTo });
}

export function parsePageSourceMap(source: string): PageSourceMap {
  const lines = splitSourceLines(source);
  const starts: BlockStart[] = [];

  for (const line of lines) {
    const start = matchSourceBlockStart(line.text);
    if (!start) continue;
    starts.push({
      ...start,
      lineIndex: line.index,
      depth: countSourceIndent(start.indent),
    });
  }

  const blocks: SourceBlock[] = [];
  const idLines: SourcePropertyLine[] = [];
  const nextPeerStartIndexes = new Array<number | undefined>(starts.length);
  const openStartIndexes: number[] = [];

  for (let startIndex = 0; startIndex < starts.length; startIndex += 1) {
    const start = starts[startIndex];
    while (
      openStartIndexes.length > 0
      && starts[openStartIndexes[openStartIndexes.length - 1]].depth >= start.depth
    ) {
      nextPeerStartIndexes[openStartIndexes.pop()!] = startIndex;
    }
    openStartIndexes.push(startIndex);
  }

  for (let startIndex = 0; startIndex < starts.length; startIndex += 1) {
    const start = starts[startIndex];
    const line = lines[start.lineIndex];
    const nextStart = starts[startIndex + 1];
    const nextStartLineIndex = nextStart?.lineIndex ?? lines.length;
    const nextPeerStart = starts[nextPeerStartIndexes[startIndex] ?? -1];

    let ownEndLineIndex = start.lineIndex;
    for (let lineIndex = start.lineIndex + 1; lineIndex < nextStartLineIndex; lineIndex += 1) {
      const candidate = lines[lineIndex];
      const candidateDepth = countSourceIndent(candidate.text);
      if (candidateDepth <= start.depth) break;
      ownEndLineIndex = lineIndex;
    }

    let subtreeEndLineIndex = (nextPeerStart?.lineIndex ?? lines.length) - 1;
    while (
      subtreeEndLineIndex > start.lineIndex &&
      lines[subtreeEndLineIndex]?.text.trim() === ""
    ) {
      subtreeEndLineIndex -= 1;
    }

    const contentSegments: SourceContentSegment[] = [];
    const propertyLines: SourcePropertyLine[] = [];
    let idLine: SourcePropertyLine | null = null;
    let id: string | null = null;
    let content = "";
    let insideCodeFence = false;

    const appendContentSegment = (segmentLine: SourceLine, sourceFrom: number, sourceTo: number, text: string) => {
      if (contentSegments.length > 0) content += "\n";
      const contentFrom = content.length;
      content += text;
      contentSegments.push({
        line: segmentLine,
        from: sourceFrom,
        to: sourceTo,
        contentFrom,
        contentTo: content.length,
        text,
      });
    };

    appendContentSegment(line, line.from + start.contentColumn, line.to, start.content);
    if (isFenceMarker(start.content)) insideCodeFence = !insideCodeFence;

    for (let lineIndex = start.lineIndex + 1; lineIndex <= ownEndLineIndex; lineIndex += 1) {
      const current = lines[lineIndex];
      const continuation = stripContinuation(current.text, start.depth + 1);
      const property = !insideCodeFence ? matchProperty(continuation.text) : null;

      if (property) {
        const propertyLine: SourcePropertyLine = {
          line: current,
          from: current.from,
          to: current.to,
          key: property.key,
          value: property.value,
        };
        propertyLines.push(propertyLine);
        if (property.key === "id") {
          id = property.value;
          idLine = propertyLine;
          idLines.push(propertyLine);
        }
        continue;
      }

      appendContentSegment(current, current.from + continuation.column, current.to, continuation.text);
      if (isFenceMarker(continuation.text)) insideCodeFence = !insideCodeFence;
    }

    const lastContentSegment = contentSegments[contentSegments.length - 1];
    blocks.push({
      id,
      depth: start.depth,
      indent: start.indent,
      line,
      ownFrom: line.from,
      ownTo: lines[ownEndLineIndex]?.to ?? line.to,
      subtreeFrom: line.from,
      subtreeTo: lines[subtreeEndLineIndex]?.to ?? line.to,
      contentFrom: line.from + start.contentColumn,
      contentTo: lastContentSegment?.to ?? line.to,
      previewFrom: line.from,
      previewTo: lastContentSegment?.to ?? line.to,
      content,
      contentSegments,
      propertyLines,
      idLine,
    });
  }

  return {
    source,
    lines,
    blocks,
    idLines,
  };
}

export function findSourceBlockAtPosition(map: PageSourceMap, position: number): SourceBlock | null {
  return map.blocks.find((block) => position >= block.ownFrom && position <= block.ownTo) ?? null;
}

export function blockIntersectsSourceRange(block: SourceBlock, from: number, to: number): boolean {
  return sourceRangesIntersect(block.ownFrom, block.ownTo, from, to);
}

export function sourceBlockContentReplacement(block: SourceBlock, content: string): SourceReplacement {
  const firstPrefix = block.line.text.slice(0, block.contentFrom - block.line.from);
  const secondSegment = block.contentSegments[1];
  const continuationPrefix = secondSegment
    ? secondSegment.line.text.slice(0, secondSegment.from - secondSegment.line.from)
    : `${block.indent}${block.indent.includes("\t") ? "\t" : "  "}`;
  const lines = content.split("\n");
  const renderedLines = lines
    .map((line, index) => `${index === 0 ? firstPrefix : continuationPrefix}${line}`)
  const propertyLinesInsideReplacement = block.propertyLines
    .filter((property) => property.from >= block.previewFrom && property.to <= block.previewTo)
    .map((property) => property.line.text);
  if (propertyLinesInsideReplacement.length > 0) {
    renderedLines.splice(1, 0, ...propertyLinesInsideReplacement);
  }
  const insert = renderedLines.join("\n");

  return {
    from: block.previewFrom,
    to: block.previewTo,
    insert,
  };
}
