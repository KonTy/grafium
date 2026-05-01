<script lang="ts">
  import BlockEditor from "./BlockEditor.svelte";
  import { listBlocks, createBlock, deleteBlock, updateBlock, moveBlock, getBacklinks } from "../lib/api";
  import type { Block, Page } from "../lib/api";
  import { pushUndo, setUndoCallback, removeUndoCallback, performUndo, performRedo, canUndo, getUndoStackSize } from "../lib/undoStack";
  import type { UndoAction } from "../lib/undoStack";

  interface Props {
    page: Page;
    compact?: boolean;
  }

  let { page, compact = false }: Props = $props();

  let blocks: Block[] = $state([]);
  let focusedBlockId: string | null = $state(null);
  let navigatingBlock = false;
  let backlinks: { link: unknown; block: Block }[] = $state([]);
  let loadError: string | null = $state(null);
  let selectedBlockIds: Set<string> = $state(new Set());
  let undoCount = $state(0);

  function updateUndoCount() {
    undoCount = getUndoStackSize();
  }

  // Listen for app-undo/app-redo custom DOM events (dispatched by main.ts)
  $effect(() => {
    const handleUndo = async () => {
      console.log("[PageContent] performing app-level undo");
      await performUndo();
      updateUndoCount();
    };
    const handleRedo = async () => {
      console.log("[PageContent] performing app-level redo");
      await performRedo();
      updateUndoCount();
    };
    window.addEventListener("app-undo", handleUndo);
    window.addEventListener("app-redo", handleRedo);
    return () => {
      window.removeEventListener("app-undo", handleUndo);
      window.removeEventListener("app-redo", handleRedo);
    };
  });

  // Register undo callback for THIS page (supports multiple instances in journal view)
  $effect(() => {
    if (page?.id) {
      setUndoCallback(page.id, (_action: UndoAction) => {
        loadBlocks();
        updateUndoCount();
      });
      return () => {
        removeUndoCallback(page.id);
      };
    }
  });

  // Load blocks when page changes
  $effect(() => {
    if (page?.id) {
      loadBlocks();
      loadBacklinks();
    }
  });

  async function loadBlocks() {
    try {
      loadError = null;
      blocks = await listBlocks(page.id);
      // If no blocks exist, create an empty one
      if (blocks.length === 0) {
        const newBlock = await createBlock(page.id, null, 0, "");
        blocks = [newBlock];
      }
    } catch (e: any) {
      loadError = e?.toString() || "Unknown error loading blocks";
      console.error("loadBlocks failed:", e);
    }
  }

  async function loadBacklinks() {
    try {
      backlinks = await getBacklinks(page.id);
    } catch {
      backlinks = [];
    }
  }

  // Track block content before editing starts, so undo can restore it
  let preEditSnapshots: Map<string, Block> = new Map();

  function handleFocus(blockId: string) {
    focusedBlockId = blockId;
    selectedBlockIds = new Set();
    // Snapshot the block content before the user edits it
    const block = blocks.find((b) => b.id === blockId);
    if (block) {
      preEditSnapshots.set(blockId, { ...block });
    }
  }

  function handleBlur(blockId: string) {
    focusedBlockId = null;
    // Don't auto-delete if we're navigating to another block
    if (navigatingBlock) {
      navigatingBlock = false;
      preEditSnapshots.delete(blockId);
      return;
    }
    preEditSnapshots.delete(blockId);
  }

  function getBlockDepth(block: Block): number {
    let depth = 0;
    let parentId = block.parent_id;
    while (parentId) {
      depth++;
      const parent = blocks.find((b) => b.id === parentId);
      parentId = parent?.parent_id ?? null;
    }
    return depth;
  }

  async function handleEnter(blockId: string, _content: string, orderIndex: number) {
    try {
      const block = blocks.find((b) => b.id === blockId);
      if (!block) return;

      let parentId: string | null;
      let newOrder: number;

      if (block.parent_id === null) {
        // Top-level block: create a child under it
        parentId = blockId;
        newOrder = blocks.filter((b) => b.parent_id === blockId).length;
      } else {
        // Already a child: create a sibling (same parent)
        parentId = block.parent_id;
        const siblings = blocks.filter((b) => b.parent_id === block.parent_id);
        const myIdx = siblings.findIndex((b) => b.id === blockId);
        newOrder = myIdx + 1;
      }

      const newBlock = await createBlock(page.id, parentId, newOrder, "");
      // Insert after current block in the array
      const idx = blocks.findIndex((b) => b.id === blockId);
      blocks = [...blocks.slice(0, idx + 1), newBlock, ...blocks.slice(idx + 1)];
      // Focus the new block
      requestAnimationFrame(() => {
        focusedBlockId = newBlock.id;
        const el = document.querySelector(`[data-block-id="${newBlock.id}"] .block-content`);
        if (el) {
          el.scrollIntoView({ block: "nearest" });
          (el as HTMLElement).click();
        }
      });
    } catch (e) {
      console.error("Failed to create block:", e);
    }
  }

  async function handlePasteBlocks(blockId: string, pasteBlocks: import("../lib/htmlToMd").PasteBlock[]) {
    try {
      const idx = blocks.findIndex((b) => b.id === blockId);
      const block = blocks[idx];
      if (!block) return;

      const baseParentId = block.parent_id;
      const newBlocks: Block[] = [];
      // Track parent at each depth level. depth 0 siblings share baseParentId,
      // depth 1+ items are children of the last block at depth-1.
      const parentAtDepth: (string | null)[] = [baseParentId];
      const orderAtDepth: number[] = [0];

      for (const pb of pasteBlocks) {
        const depth = pb.depth;
        // Determine parent: if depth > 0, parent is the last block at depth-1
        const parentId = depth > 0 ? (parentAtDepth[depth] ?? parentAtDepth[parentAtDepth.length - 1] ?? baseParentId) : baseParentId;

        // Get order index for this depth
        if (!orderAtDepth[depth]) orderAtDepth[depth] = 0;
        const order = orderAtDepth[depth]!;
        orderAtDepth[depth] = order + 1;

        const newBlock = await createBlock(page.id, parentId, order, pb.content);
        newBlocks.push(newBlock);

        // This block can be a parent for deeper items
        parentAtDepth[depth + 1] = newBlock.id;
        // Reset child order counters for deeper levels
        for (let d = depth + 1; d < orderAtDepth.length; d++) {
          orderAtDepth[d] = 0;
        }
      }
      // Insert all new blocks after the current block
      blocks = [...blocks.slice(0, idx + 1), ...newBlocks, ...blocks.slice(idx + 1)];
      // Focus the last new block
      const lastNew = newBlocks[newBlocks.length - 1];
      requestAnimationFrame(() => {
        focusedBlockId = lastNew.id;
        const el = document.querySelector(`[data-block-id="${lastNew.id}"] .block-content`);
        if (el) {
          el.scrollIntoView({ block: "nearest" });
          (el as HTMLElement).click();
        }
      });
    } catch (e) {
      console.error("Failed to paste blocks:", e);
    }
  }

  async function handleClickBelow() {
    try {
      const lastOrder = blocks.length > 0 ? blocks[blocks.length - 1].order_index + 1 : 0;
      const newBlock = await createBlock(page.id, null, lastOrder, "");
      blocks = [...blocks, newBlock];
      requestAnimationFrame(() => {
        focusedBlockId = newBlock.id;
        const el = document.querySelector(`[data-block-id="${newBlock.id}"] .block-content`);
        if (el) (el as HTMLElement).click();
      });
    } catch (e) {
      console.error("Failed to create block:", e);
    }
  }

  async function handleDelete(blockId: string) {
    console.log("[DELETE] handleDelete called, blockId:", blockId, "total blocks:", blocks.length);
    if (blocks.length <= 1) { console.log("[DELETE] skipping - only 1 block left"); return; }
    const block = blocks.find((b) => b.id === blockId);
    if (block) {
      // Use pre-edit snapshot if available (has original content before clearing)
      const snapshot = preEditSnapshots.get(blockId) || block;
      console.log("[DELETE] pushing to undo stack, content:", snapshot.content.substring(0, 40));
      pushUndo({ type: "delete_blocks", blocks: [snapshot], pageId: page.id });
      updateUndoCount();
      preEditSnapshots.delete(blockId);
    } else {
      console.log("[DELETE] block not found!");
    }
    await deleteBlock(blockId);
    const idx = blocks.findIndex((b) => b.id === blockId);
    blocks = blocks.filter((b) => b.id !== blockId);
    // Focus previous block
    const prevIdx = Math.max(0, idx - 1);
    if (blocks[prevIdx]) {
      requestAnimationFrame(() => {
        const el = document.querySelector(`[data-block-id="${blocks[prevIdx].id}"] .block-content`);
        if (el) {
          el.scrollIntoView({ block: "nearest" });
          (el as HTMLElement).click();
        }
      });
    }
  }

  function handleNavigate(blockId: string, direction: "up" | "down") {
    navigatingBlock = true;
    const idx = blocks.findIndex((b) => b.id === blockId);
    const targetIdx = direction === "up" ? idx - 1 : idx + 1;
    if (targetIdx >= 0 && targetIdx < blocks.length) {
      const blockEl = document.querySelector(`[data-block-id="${blocks[targetIdx].id}"]`);
      const contentEl = blockEl?.querySelector(".block-content");
      if (blockEl && contentEl) {
        blockEl.scrollIntoView({ block: "nearest" });
        (contentEl as HTMLElement).click();
      }
    }
  }

  async function handleIndent(blockId: string, direction: "in" | "out") {
    const idx = blocks.findIndex((b) => b.id === blockId);
    const block = blocks[idx];
    if (!block) return;

    try {
      if (direction === "in") {
        // Indent: become a child of the previous sibling at the same level
        // Find previous sibling (same parent_id, appears before in list)
        const prevSibling = [...blocks].slice(0, idx).reverse().find(
          (b) => b.parent_id === block.parent_id
        );
        if (!prevSibling) return; // Can't indent if no previous sibling

        // Count existing children of prevSibling to get order_index
        const childCount = blocks.filter((b) => b.parent_id === prevSibling.id).length;
        await moveBlock(block.id, prevSibling.id, childCount);
        block.parent_id = prevSibling.id;
        block.order_index = childCount;
        blocks = [...blocks];
      } else {
        // Outdent: become a sibling of the current parent
        if (!block.parent_id) return; // Already at top level

        const parent = blocks.find((b) => b.id === block.parent_id);
        if (!parent) return;

        // New parent is the grandparent (or null for top level)
        const newParentId = parent.parent_id ?? null;
        // Place after the parent in order
        const siblingsOfParent = blocks.filter((b) => b.parent_id === newParentId);
        const parentOrder = siblingsOfParent.findIndex((b) => b.id === parent.id);
        const newOrder = parentOrder + 1;

        // Shift siblings after insertion point
        await moveBlock(block.id, newParentId, newOrder);
        block.parent_id = newParentId;
        block.order_index = newOrder;
        blocks = [...blocks];
      }
    } catch (e) {
      console.error("Failed to indent/outdent:", e);
    }
  }

  function handleBulletClick(blockId: string, event: MouseEvent) {
    if (event.shiftKey && selectedBlockIds.size > 0) {
      // Range select from last selected to this block
      const lastSelected = [...selectedBlockIds].pop()!;
      const startIdx = blocks.findIndex((b) => b.id === lastSelected);
      const endIdx = blocks.findIndex((b) => b.id === blockId);
      const [from, to] = startIdx < endIdx ? [startIdx, endIdx] : [endIdx, startIdx];
      const newSelection = new Set(selectedBlockIds);
      for (let i = from; i <= to; i++) {
        newSelection.add(blocks[i].id);
      }
      selectedBlockIds = newSelection;
    } else {
      // Toggle single block selection
      const newSelection = new Set(selectedBlockIds);
      if (newSelection.has(blockId)) {
        newSelection.delete(blockId);
      } else {
        newSelection.add(blockId);
      }
      selectedBlockIds = newSelection;
    }
    // Clear any active editor focus
    if (focusedBlockId) {
      focusedBlockId = null;
    }
  }

  async function handleDeleteSelected() {
    if (selectedBlockIds.size === 0) return;
    const toDelete = [...selectedBlockIds];

    // Save deleted blocks for undo
    const deletedBlocks = blocks.filter((b) => selectedBlockIds.has(b.id));
    pushUndo({ type: "delete_blocks", blocks: deletedBlocks, pageId: page.id });
    updateUndoCount();

    for (const id of toDelete) {
      await deleteBlock(id);
    }

    const remaining = blocks.filter((b) => !selectedBlockIds.has(b.id));
    if (remaining.length === 0) {
      // All blocks deleted — create a fresh empty block
      const newBlock = await createBlock(page.id, null, 0, "");
      blocks = [newBlock];
    } else {
      blocks = remaining;
    }
    selectedBlockIds = new Set();
  }

  function handleKeydownForSelection(e: KeyboardEvent) {
    if (selectedBlockIds.size === 0) return;
    if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      handleDeleteSelected();
    } else if (e.key === "Escape") {
      selectedBlockIds = new Set();
    }
  }
</script>

<svelte:window onkeydown={handleKeydownForSelection} />

<div class="page-content" class:compact>
  <h1 class="page-title">{page.title}</h1>

  {#if loadError}
    <div class="load-error" style="color: #f38ba8; background: #1e1e2e; padding: 12px; border-radius: 8px; margin-bottom: 16px; font-family: monospace; font-size: 13px; white-space: pre-wrap;">
      Error: {loadError}
    </div>
  {/if}

  <div class="blocks-container">
    {#each blocks as block (block.id)}
      <div data-block-id={block.id}>
        <BlockEditor
          {block}
          pageId={page.id}
          depth={getBlockDepth(block)}
          focused={focusedBlockId === block.id}
          selected={selectedBlockIds.has(block.id)}
          onFocus={handleFocus}
          onBlur={handleBlur}
          onEnter={handleEnter}
          onDelete={handleDelete}
          onNavigate={handleNavigate}
          onIndent={handleIndent}
          onBulletClick={handleBulletClick}
          onPasteBlocks={handlePasteBlocks}
        />
      </div>
    {/each}
  </div>

  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="click-below" onclick={handleClickBelow}></div>

  {#if backlinks.length > 0}
    <div class="backlinks-section">
      <h3 class="backlinks-title">{backlinks.length} Linked Reference{backlinks.length > 1 ? "s" : ""}</h3>
      <div class="backlinks-list">
        {#each backlinks as bl}
          <div class="backlink-item">
            <span class="backlink-content">{@html bl.block.content}</span>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .page-content {
    max-width: var(--content-max-width);
    margin: 0 auto;
    padding: var(--content-padding-y) var(--content-padding-x);
  }

  .page-title {
    font-size: 32px;
    font-weight: 700;
    margin-bottom: 8px;
    color: var(--text-primary);
  }

  .blocks-container {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .click-below {
    min-height: 300px;
    cursor: text;
  }

  .compact {
    padding: 0;
  }

  .compact .click-below {
    min-height: 0;
  }

  .backlinks-section {
    margin-top: 48px;
    padding-top: 24px;
    border-top: 1px solid var(--border);
  }

  .backlinks-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-secondary);
    margin-bottom: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .backlinks-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .backlink-item {
    padding: 8px 12px;
    background: var(--bg-secondary);
    border-radius: 6px;
    font-size: 14px;
  }
</style>
