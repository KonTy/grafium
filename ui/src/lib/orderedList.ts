import type { Block } from "./api";

// Twelve nine-digit segments, separators, indent, delimiter and whitespace.
export const ORDERED_MARKER_LOOKAHEAD = 128;

export function parseOrderedListMarker(line: string) {
  const match = /^( {0,3})(\d{1,9}(?:\.\d{1,9}){0,11})([.)])([ \t]+|$)/.exec(line);
  if (!match) return null;
  const [, indent, digits, delimiter, spacing] = match;
  return { prefix: match[0], numbers: digits.split(".").map(Number), indent, delimiter, spacing };
}

function prefixFor(marker: NonNullable<ReturnType<typeof parseOrderedListMarker>>, numbers: number[]) {
  return `${marker.indent}${numbers.join(".")}${marker.delimiter}${marker.spacing || " "}`;
}

export function orderedListMarker(line: string): { prefix: string; nextPrefix: string | null } | null {
  const marker = parseOrderedListMarker(line);
  if (!marker) return null;
  const next = [...marker.numbers];
  next[next.length - 1] += 1;
  return {
    prefix: marker.prefix,
    nextPrefix: next.at(-1)! < 1_000_000_000 ? prefixFor(marker, next) : null,
  };
}

/** Re-label only a moved numbered branch, never its old or new siblings. */
export function movedOrderedListChanges(
  tree: {
    blockById: ReadonlyMap<string, Block>;
    childrenByParent: ReadonlyMap<string | null, readonly Block[]>;
  },
  blockId: string, oldParentId: string | null, direction: "in" | "out",
): { blockId: string; beforeContent: string; afterContent: string }[] {
  const { blockById: byId, childrenByParent: children } = tree;
  const root = byId.get(blockId);
  const marker = root && parseOrderedListMarker(root.content);
  if (!root || !marker) return [];
  const parent = root.parent_id ? byId.get(root.parent_id) : null;
  const parentMarker = parent && parseOrderedListMarker(parent.content);
  if (direction === "in" && !parentMarker) return [];
  if (direction === "out" && !parseOrderedListMarker(byId.get(oldParentId ?? "")?.content ?? "")) return [];

  const siblings = children.get(root.parent_id) ?? [];
  const preceding = siblings.slice(0, siblings.findIndex((block) => block.id === root.id));
  const parentNumbers = parentMarker?.numbers ?? [];
  let numbers = [...parentNumbers, 1];
  for (const sibling of preceding) {
    const candidate = parseOrderedListMarker(sibling.content);
    if (candidate && candidate.numbers.length === parentNumbers.length + 1
      && parentNumbers.every((number, index) => candidate.numbers[index] === number)) {
      numbers = [...candidate.numbers];
      numbers[numbers.length - 1] += 1;
    }
  }
  if (numbers.length > 12 || numbers.at(-1)! >= 1_000_000_000) return [];
  const changes: { blockId: string; beforeContent: string; afterContent: string }[] = [];
  function visit(block: Block, path: number[]) {
    const current = parseOrderedListMarker(block.content);
    if (!current || path.length > 12) return;
    const afterContent = prefixFor(current, path) + block.content.slice(current.prefix.length);
    if (afterContent !== block.content) {
      changes.push({ blockId: block.id, beforeContent: block.content, afterContent });
    }
    for (const child of children.get(block.id) ?? []) {
      const childMarker = parseOrderedListMarker(child.content);
      if (childMarker) visit(child, [...path, childMarker.numbers.at(-1)!]);
    }
  }
  visit(root, numbers);
  return changes;
}
