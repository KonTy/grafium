import { marked } from "marked";

export const READING_NOTE_OPEN = "<!-- grafium-reading-note ";
export const READING_NOTE_CLOSE = "<!-- /grafium-reading-note -->";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export interface ManagedNoteMetadata {
  version: number;
  id: string;
  bodyBlockId: string;
  footnoteLabel?: string | null;
  [key: string]: unknown;
}

export interface ManagedNoteRange {
  metadata: ManagedNoteMetadata;
  from: number;
  to: number;
  firstLine: number;
  lastLine: number;
  bodyFirstLine: number;
  bodyLastLine: number;
  firstPrefix: string;
  opening: string;
}

export function validReadingNoteLabel(label: unknown): label is string {
  return typeof label === "string" && /^grafium-note-[1-9]\d*$/.test(label)
    && BigInt(label.slice("grafium-note-".length)) <= 18446744073709551615n;
}

function parseMetadata(value: string, version: 1 | 2): ManagedNoteMetadata | null {
  try {
    const metadata = JSON.parse(value) as ManagedNoteMetadata;
    return metadata && metadata.version === version && UUID.test(metadata.id) && UUID.test(metadata.bodyBlockId)
      && (version === 1 || validReadingNoteLabel(metadata.footnoteLabel)) ? metadata : null;
  } catch {
    return null;
  }
}

export function legacyReadingNoteBody(source: string): { metadata: ManagedNoteMetadata; bodyFrom: number } | null {
  let offset = 0;
  let metadata: ManagedNoteMetadata | null = null;
  const keys = new Set<string>();
  for (const line of source.split("\n")) {
    offset += line.length + 1;
    if (!line.replace(/\r$/, "")) return metadata ? { metadata, bodyFrom: Math.min(offset, source.length) } : null;
    const match = /^([a-zA-Z_-]+)::(.*)\r?$/.exec(line);
    if (!match || keys.has(match[1])) return null;
    keys.add(match[1]);
    if (match[1] === "reading-note") metadata = parseMetadata(match[2].trim(), 1);
  }
  return null;
}

export function managedReadingNoteRanges(source: string): ManagedNoteRange[] {
  if (!source.includes(READING_NOTE_OPEN)) return [];
  const htmlStarts = new Set<number>();
  let tokenOffset = 0;
  for (const token of marked.lexer(source)) {
    if (token.type === "html") htmlStarts.add(tokenOffset);
    tokenOffset += token.raw.length;
  }

  const lines = source.split("\n");
  const offsets: number[] = [];
  let offset = 0;
  for (const line of lines) { offsets.push(offset); offset += line.length + 1; }
  const notes: ManagedNoteRange[] = [];
  for (let index = 0; index < lines.length; index++) {
    const opening = lines[index].replace(/\r$/, "");
    if (!htmlStarts.has(offsets[index]) || !opening.startsWith(READING_NOTE_OPEN) || !opening.endsWith(" -->")) continue;
    const metadata = parseMetadata(opening.slice(READING_NOTE_OPEN.length, -4), 2);
    if (!metadata) continue;
    const prefix = `[^${metadata.footnoteLabel}]:`;
    const definition = lines[index + 1]?.replace(/\r$/, "");
    if (!definition?.startsWith(prefix)) continue;
    const firstPrefix = prefix + (definition[prefix.length] === " " ? " " : "");
    let end = index + 2;
    while (end < lines.length && lines[end].replace(/\r$/, "") !== READING_NOTE_CLOSE) {
      if (!lines[end].startsWith("    ")) break;
      end++;
    }
    if (lines[end]?.replace(/\r$/, "") !== READING_NOTE_CLOSE) continue;
    notes.push({
      metadata, from: offsets[index], to: Math.min(source.length, offsets[end] + lines[end].length + 1),
      firstLine: index, lastLine: end, bodyFirstLine: index + 1, bodyLastLine: end - 1, firstPrefix, opening,
    });
    index = end;
  }
  return notes;
}

export function touchesReadingNoteDefinition(source: string, changes: { from: number; to: number }[]): boolean {
  const notes = managedReadingNoteRanges(source);
  return changes.some((change) => notes.some((note) => change.from === change.to
    ? change.from > note.from && change.from < note.to
    : change.from < note.to && change.to > note.from));
}

export function readingNoteBlockLabel(block: { id: string; properties: Record<string, unknown> }): string | null {
  if (block.properties["reading-note-storage"] !== "inline") return null;
  const label = block.properties["reading-note-label"];
  const metadata = typeof block.properties["reading-note"] === "string"
    ? parseMetadata(block.properties["reading-note"], 2) : null;
  return validReadingNoteLabel(label) && metadata?.bodyBlockId === block.id && metadata.footnoteLabel === label ? label : null;
}

export function isManagedReadingNoteBlock(block: { id: string; properties: Record<string, unknown> }): boolean {
  const value = block.properties["reading-note"];
  if (typeof value !== "string") return false;
  const metadata = parseMetadata(value, 2) ?? parseMetadata(value, 1);
  return metadata?.bodyBlockId === block.id;
}

/** Prompt projection only. Keep the original blocks for concurrency checks and persistence. */
export function readingSourceBlocks<T extends { id: string; parent_id: string | null; properties: Record<string, unknown> }>(blocks: T[]): T[] {
  const children = new Map<string, string[]>();
  const excluded = new Set(blocks.filter(isManagedReadingNoteBlock).map((block) => block.id));
  for (const block of blocks) {
    if (!block.parent_id) continue;
    const siblings = children.get(block.parent_id) ?? [];
    siblings.push(block.id);
    children.set(block.parent_id, siblings);
  }
  const pending = [...excluded];
  for (let index = 0; index < pending.length; index++) {
    for (const child of children.get(pending[index]) ?? []) {
      if (excluded.has(child)) continue;
      excluded.add(child);
      pending.push(child);
    }
  }
  return blocks.filter((block) => !excluded.has(block.id));
}
