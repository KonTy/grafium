import { createBlocks, deleteBlocks, listBlocks, updateBlock, type Block, type CreateBlockBatchItem } from "./api";
import {
  notifyBlockSelectionChanged,
  pushUndo,
  runUndoOperation,
  type DeleteBlockSelectionAction,
} from "./undoStack";

export interface SelectionBlockGroup {
  pageId: string;
  blocks: Block[];
}

function canonicalJson(value: unknown): string | undefined {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.entries(value).sort(([a], [b]) => a.localeCompare(b))
      .map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item)}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function sameSnapshot(a: Block, b: Block): boolean {
  return a.page_id === b.page_id
    && a.parent_id === b.parent_id
    && a.order_index === b.order_index
    && a.content === b.content
    && a.block_type === b.block_type
    && canonicalJson(a.properties) === canonicalJson(b.properties);
}

function normalizeGroups(groups: readonly SelectionBlockGroup[]): SelectionBlockGroup[] {
  const pages = new Map<string, Map<string, Block>>();
  const byId = new Map<string, Block>();
  for (const { pageId, blocks } of groups) {
    if (!pageId) throw new Error("A selected page has no ID.");
    for (const block of blocks) {
      if (!block.id || block.page_id !== pageId) {
        throw new Error(`Selected block ${block.id || "(missing ID)"} does not belong to page ${pageId}.`);
      }
      const previous = byId.get(block.id);
      if (previous) {
        if (!sameSnapshot(previous, block)) throw new Error(`Conflicting snapshots for selected block ${block.id}.`);
        continue;
      }
      // Svelte may supply proxied properties; JSON is also the native API's format.
      const snapshot: Block = JSON.parse(JSON.stringify(block));
      byId.set(block.id, snapshot);
      if (!pages.has(pageId)) pages.set(pageId, new Map());
      pages.get(pageId)!.set(block.id, snapshot);
    }
  }
  for (const block of byId.values()) {
    const seen = new Set([block.id]);
    let parent = block.parent_id ? byId.get(block.parent_id) : undefined;
    while (parent) {
      if (seen.has(parent.id) || parent.page_id !== block.page_id) {
        throw new Error(`Invalid parent relationship for selected block ${block.id}.`);
      }
      seen.add(parent.id);
      parent = parent.parent_id ? byId.get(parent.parent_id) : undefined;
    }
  }
  return [...pages].map(([pageId, blocks]) => ({ pageId, blocks: [...blocks.values()] }));
}

function assertCompleteSubtrees(group: SelectionBlockGroup, current: Block[]): void {
  const ids = new Set(group.blocks.map((block) => block.id));
  if (current.some((block) => block.parent_id && ids.has(block.parent_id) && !ids.has(block.id))) {
    throw new Error("The selected outline has changed. Select the blocks again before deleting.");
  }
}

function restorationItems(blocks: Block[]): CreateBlockBatchItem[] {
  const byId = new Map(blocks.map((block) => [block.id, block]));
  const ordered: Block[] = [];
  const visited = new Set<string>();
  function visit(block: Block) {
    if (visited.has(block.id)) return;
    visited.add(block.id);
    const parent = block.parent_id ? byId.get(block.parent_id) : undefined;
    if (parent) visit(parent);
    ordered.push(block);
  }
  [...blocks].sort((a, b) => a.order_index - b.order_index).forEach(visit);
  return ordered.map((block) => ({
    id: block.id,
    parentId: block.parent_id,
    orderIndex: block.order_index,
    content: block.content,
    blockType: block.block_type.toLowerCase(),
    properties: block.properties,
  }));
}

function isUntouchedPlaceholder(block: Block, current: Block[]): boolean {
  return block.content === ""
    && block.parent_id === null
    && block.order_index === 0
    && block.block_type.toLowerCase() === "text"
    && Object.keys(block.properties).length === 0
    && !current.some((child) => child.parent_id === block.id);
}

export async function applyBlockSelection(
  action: DeleteBlockSelectionAction,
  direction: "delete" | "restore",
): Promise<void> {
  try {
    for (const group of action.groups) {
      let current = await listBlocks(group.pageId);
      const existingIds = new Set(current.map((block) => block.id));
      let wrotePage = false;
      if (direction === "restore") {
        const missing = group.blocks.filter((block) => !existingIds.has(block.id));
        const missingIds = new Set(missing.map((block) => block.id));
        for (const block of missing) {
          if (block.parent_id && !existingIds.has(block.parent_id)
            && !missingIds.has(block.parent_id)) {
            throw new Error(`Cannot restore block ${block.id}: its parent is missing.`);
          }
        }
        if (missing.length) {
          await createBlocks(group.pageId, restorationItems(missing));
          wrotePage = true;
        }
        current = await listBlocks(group.pageId);
        const restoredIds = new Set(current.map((block) => block.id));
        if (group.blocks.some((block) => !restoredIds.has(block.id))) {
          throw new Error(`Some selected blocks could not be restored on page ${group.pageId}.`);
        }
        const placeholder = current.find((block) => block.id === action.placeholderIds[group.pageId]);
        if (placeholder && isUntouchedPlaceholder(placeholder, current)) {
          await deleteBlocks(group.pageId, [placeholder.id]);
          wrotePage = true;
          current = current.filter((block) => block.id !== placeholder.id);
        }
      } else {
        assertCompleteSubtrees(group, current);
        const ids = group.blocks.filter((block) => existingIds.has(block.id)).map((block) => block.id);
        const selectedIds = new Set(ids);
        if (current.every((block) => selectedIds.has(block.id))) {
          // Create before deleting so reloads never invent an untracked empty block.
          const placeholderId = action.placeholderIds[group.pageId] ?? crypto.randomUUID();
          action.placeholderIds[group.pageId] = placeholderId;
          await createBlocks(group.pageId, [{
            id: placeholderId,
            parentId: null,
            orderIndex: 0,
            content: "",
            blockType: "text",
            properties: {},
          }]);
          wrotePage = true;
        }
        if (ids.length) {
          await deleteBlocks(group.pageId, ids);
          wrotePage = true;
        }
        current = await listBlocks(group.pageId);
        if (current.some((block) => selectedIds.has(block.id))) {
          throw new Error(`Some selected blocks could not be deleted on page ${group.pageId}.`);
        }
      }

      // Native batches can commit rows and then fail writing Markdown. Even if a
      // retry has no missing IDs, a successful full-page write must still occur.
      const survivor = current[0];
      if (!survivor) throw new Error(`Page ${group.pageId} unexpectedly has no blocks.`);
      if (!wrotePage) await updateBlock(survivor.id, survivor.content, survivor.properties);
    }
  } finally {
    notifyBlockSelectionChanged(action);
  }
}

/** Delete saved, complete subtree snapshots as one recoverable cross-page action. */
export async function deleteBlockSelection(groups: readonly SelectionBlockGroup[]): Promise<void> {
  return runUndoOperation(async () => {
    const snapshots = normalizeGroups(groups);
    if (!snapshots.length) return;
    for (const group of snapshots) {
      const current = await listBlocks(group.pageId);
      const byId = new Map(current.map((block) => [block.id, block]));
      if (group.blocks.some((block) => !byId.has(block.id) || !sameSnapshot(block, byId.get(block.id)!))) {
        throw new Error("The selected blocks have changed. Save and select them again before deleting.");
      }
      assertCompleteSubtrees(group, current);
    }
    const action: DeleteBlockSelectionAction = {
      type: "delete_block_selection",
      pageId: snapshots[0].pageId,
      groups: snapshots,
      placeholderIds: Object.create(null),
    };
    // Secure every snapshot before the first native call can partially mutate DB.
    pushUndo(action);
    try {
      await applyBlockSelection(action, "delete");
    } catch (error) {
      throw new Error(`Could not finish deleting the selection. Undo can restore affected blocks: ${error instanceof Error ? error.message : String(error)}`);
    }
  });
}
