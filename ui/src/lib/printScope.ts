/**
 * What gets printed, worked out from stored blocks rather than from the DOM.
 *
 * The rendered page cannot be trusted as a source: long pages mount only a
 * window of blocks, collapsed subtrees are left out entirely, a focused block
 * shows CodeMirror instead of rendered content, and journal days can still be
 * placeholders. Printing any of that would quietly drop the user's writing, so
 * every scope here is resolved from the full block list instead.
 */

import type { Block } from "./api";
import { getHeadingLevel } from "./blockLayout";

export type PrintScopeKind = "page" | "chapter" | "selection";

/** A block and how deeply it is nested, in reading order. */
export interface PrintBlock {
  block: Block;
  depth: number;
}

/** A heading offered in the print dialog's chapter picker. */
export interface ChapterHeading {
  id: string;
  /** 1 for `#` through 6 for `######`. */
  level: number;
  text: string;
}

function byOrder(a: Block, b: Block): number {
  return a.order_index - b.order_index || a.id.localeCompare(b.id);
}

/**
 * Put every block in reading order with its nesting depth, ignoring fold
 * state so collapsed writing still prints.
 *
 * Blocks whose parent is missing are kept as roots rather than dropped: a
 * broken link in the tree must never silently remove someone's notes from
 * their printout. Already-visited blocks are skipped so a cycle cannot hang
 * printing.
 */
export function flattenBlocks(blocks: readonly Block[]): PrintBlock[] {
  const present = new Set(blocks.map((block) => block.id));
  const children = new Map<string, Block[]>();
  const roots: Block[] = [];
  for (const block of blocks) {
    const parent = block.parent_id;
    if (parent !== null && present.has(parent)) {
      const siblings = children.get(parent);
      if (siblings) siblings.push(block);
      else children.set(parent, [block]);
    } else {
      roots.push(block);
    }
  }
  roots.sort(byOrder);
  for (const siblings of children.values()) siblings.sort(byOrder);

  const flat: PrintBlock[] = [];
  const seen = new Set<string>();
  const walk = (block: Block, depth: number): void => {
    if (seen.has(block.id)) return;
    seen.add(block.id);
    flat.push({ block, depth });
    for (const child of children.get(block.id) ?? []) walk(child, depth + 1);
  };
  for (const root of roots) walk(root, 0);
  // A parent cycle leaves blocks with no reachable root. Print them anyway,
  // flush left, rather than losing them.
  if (seen.size < blocks.length) {
    for (const block of [...blocks].sort(byOrder)) walk(block, 0);
  }
  return flat;
}

/** Heading text without its `#` marks or inline Markdown punctuation. */
export function headingText(content: string): string {
  return content
    .trimStart()
    .replace(/^#{1,6}\s+/, "")
    .replace(/\[\[([^\]]*)\]\]/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/(\*\*|__|~~|`|\*|_)/g, "")
    .trim();
}

/** Every heading on the page, in reading order, for the chapter picker. */
export function listChapterHeadings(flat: readonly PrintBlock[]): ChapterHeading[] {
  const headings: ChapterHeading[] = [];
  for (const { block } of flat) {
    const level = getHeadingLevel(block.content);
    if (level === 0) continue;
    headings.push({ id: block.id, level, text: headingText(block.content) || "Untitled section" });
  }
  return headings;
}

/** Ids of a block's descendants, for outlines that nest content under a heading. */
function descendantIds(flat: readonly PrintBlock[], startIndex: number): Set<string> {
  const start = flat[startIndex];
  const ids = new Set<string>();
  for (let i = startIndex + 1; i < flat.length; i += 1) {
    if (flat[i].depth <= start.depth) break;
    ids.add(flat[i].block.id);
  }
  return ids;
}

/**
 * Everything under one heading: the heading itself, anything nested beneath
 * it, and the blocks that follow until the next heading of the same or a
 * higher rank.
 *
 * One rule covers both shapes Grafium produces. Imported books are a single
 * long page where chapters are sibling `#` headings with their text following
 * them, which the "stop at the next heading of rank 1..N" clause handles.
 * Hand-written outlines indent content beneath the heading instead, and those
 * descendants are kept even if someone has nested a same-rank heading inside,
 * so picking a chapter never prints less than it visibly contains.
 */
export function chapterRange(flat: readonly PrintBlock[], headingId: string): PrintBlock[] {
  const startIndex = flat.findIndex((entry) => entry.block.id === headingId);
  if (startIndex === -1) return [];
  const level = getHeadingLevel(flat[startIndex].block.content);
  if (level === 0) return [];

  const inside = descendantIds(flat, startIndex);
  const chapter: PrintBlock[] = [flat[startIndex]];
  for (let i = startIndex + 1; i < flat.length; i += 1) {
    const entry = flat[i];
    if (!inside.has(entry.block.id)) {
      const nextLevel = getHeadingLevel(entry.block.content);
      if (nextLevel >= 1 && nextLevel <= level) break;
    }
    chapter.push(entry);
  }
  return chapter;
}

/** The chapter containing a block, so the picker can start where the user is. */
export function chapterIdForBlock(flat: readonly PrintBlock[], blockId: string): string | null {
  const index = flat.findIndex((entry) => entry.block.id === blockId);
  if (index === -1) return null;
  for (let i = index; i >= 0; i -= 1) {
    if (getHeadingLevel(flat[i].block.content) > 0) return flat[i].block.id;
  }
  return null;
}

/** Selected blocks plus everything nested under them. */
export function selectionRange(
  flat: readonly PrintBlock[],
  selectedIds: readonly string[],
): PrintBlock[] {
  const wanted = new Set(selectedIds);
  if (wanted.size === 0) return [];
  const keep = new Set<string>();
  flat.forEach((entry, index) => {
    if (!wanted.has(entry.block.id)) return;
    keep.add(entry.block.id);
    for (const id of descendantIds(flat, index)) keep.add(id);
  });
  return flat.filter((entry) => keep.has(entry.block.id));
}

/**
 * Re-level a range so its outermost blocks sit flush left, keeping the
 * relative nesting of everything below them.
 */
export function normalizeDepth(range: readonly PrintBlock[]): PrintBlock[] {
  if (range.length === 0) return [];
  const base = Math.min(...range.map((entry) => entry.depth));
  return range.map((entry) => ({ block: entry.block, depth: entry.depth - base }));
}

export interface PrintScope {
  kind: PrintScopeKind;
  /** Which heading to print, for `chapter`. */
  chapterId?: string | null;
  /** Which blocks to print, for `selection`. */
  selectedIds?: readonly string[];
}

/** The blocks a scope prints, flush left and in reading order. */
export function resolvePrintBlocks(blocks: readonly Block[], scope: PrintScope): PrintBlock[] {
  const flat = flattenBlocks(blocks);
  switch (scope.kind) {
    case "chapter":
      return normalizeDepth(scope.chapterId ? chapterRange(flat, scope.chapterId) : []);
    case "selection":
      return normalizeDepth(selectionRange(flat, scope.selectedIds ?? []));
    default:
      return flat;
  }
}
