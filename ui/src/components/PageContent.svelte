<script lang="ts">
  import BlockEditor from "./BlockEditor.svelte";
  import { listBlocks, createBlock, deleteBlock, updateBlock, moveBlock, getBacklinks } from "../lib/api";
  import type { Block, Page } from "../lib/api";

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

  function handleFocus(blockId: string) {
    focusedBlockId = blockId;
  }

  function handleBlur(blockId: string) {
    focusedBlockId = null;
    // Don't auto-delete if we're navigating to another block
    if (navigatingBlock) {
      navigatingBlock = false;
      return;
    }
    // Auto-delete empty blocks on blur (keep at least one)
    const block = blocks.find((b) => b.id === blockId);
    if (block && block.content.trim() === "" && blocks.length > 1) {
      deleteBlock(blockId);
      blocks = blocks.filter((b) => b.id !== blockId);
    }
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
    if (blocks.length <= 1) return; // Keep at least one block
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
</script>

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
          onFocus={handleFocus}
          onBlur={handleBlur}
          onEnter={handleEnter}
          onDelete={handleDelete}
          onNavigate={handleNavigate}
          onIndent={handleIndent}
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
