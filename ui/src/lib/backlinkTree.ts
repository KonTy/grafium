import type { Block } from "./api";

export type BacklinkTreeNode = { block: Block; depth: number };

export type BacklinkSourceIndex = {
  blockMap: Map<string, Block>;
  childrenByParent: Map<string | null, Block[]>;
  visible: BacklinkTreeNode[];
  visibleIndex: Map<string, number>;
};

/** Empty Logseq-style bullets (`-`, `*`, `+`, or blank) that still wrap children. */
export function isEmptyOutlineBlock(content: string): boolean {
  const normalized = content.replace(/[\u200B-\u200D\uFEFF]/g, "").trim();
  return normalized === "" || /^[-*+]$/.test(normalized);
}

function flattenVisibleOutline(childrenByParent: Map<string | null, Block[]>): BacklinkTreeNode[] {
  const visible: BacklinkTreeNode[] = [];
  const walk = (block: Block, depth: number) => {
    if (!isEmptyOutlineBlock(block.content)) {
      visible.push({ block, depth });
    }
    for (const child of childrenByParent.get(block.id) ?? []) {
      walk(child, depth + 1);
    }
  };
  for (const root of childrenByParent.get(null) ?? []) {
    walk(root, 0);
  }
  return visible;
}

export function buildBacklinkSourceIndex(sourceBlocks: Block[]): BacklinkSourceIndex {
  const blockMap = new Map(sourceBlocks.map((block) => [block.id, block]));
  const childrenByParent = new Map<string | null, Block[]>();

  for (const block of sourceBlocks) {
    const key = block.parent_id ?? null;
    const current = childrenByParent.get(key) ?? [];
    current.push(block);
    childrenByParent.set(key, current);
  }

  for (const childList of childrenByParent.values()) {
    childList.sort((a, b) => a.order_index - b.order_index);
  }

  const visible = flattenVisibleOutline(childrenByParent);
  const visibleIndex = new Map(visible.map((node, index) => [node.block.id, index]));
  return { blockMap, childrenByParent, visible, visibleIndex };
}

/**
 * Linked-reference tree: the linked block plus every visible block indented
 * under it, until the next block at the same (or shallower) level.
 *
 * Empty `-` wrappers are skipped, but their children keep their indent, so
 * journal sections that only *look* nested still belong to the page link.
 */
export function buildBacklinkTree(rootBlockId: string, index: BacklinkSourceIndex): BacklinkTreeNode[] {
  const start = index.visibleIndex.get(rootBlockId);
  if (start === undefined) return [];

  const rootDepth = index.visible[start].depth;
  const tree: BacklinkTreeNode[] = [];
  for (let i = start; i < index.visible.length; i++) {
    const node = index.visible[i];
    if (i > start && node.depth <= rootDepth) break;
    tree.push({ block: node.block, depth: node.depth - rootDepth });
  }
  return tree;
}
