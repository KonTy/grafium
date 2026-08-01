<script lang="ts">
  import { SvelteMap } from "svelte/reactivity";
  import { tick } from "svelte";
  import BlockEditor from "./BlockEditor.svelte";
  import { listBlocks, createBlock, deleteBlock, updateBlock, moveBlock, getBacklinks, getPage, getParentPage, getChildPages } from "../lib/api";
  import { persistBlockContentIfChanged } from "../lib/persistence";
  import { buildBlockRenderState, computeVirtualWindow } from "../lib/pageContentVirtualization";
  import { renderBlock } from "../lib/markdown";
  import { hydrateRenderedMedia } from "../lib/renderedMedia";
  import {
    applyIfCurrentPageLoad,
    beginPageLoad,
    capturePageLoad,
    createPageLoadState,
    isCurrentPageLoad,
    type PageLoadRequest,
  } from "../lib/pageContentLoad";
  import type { BacklinkResult, Block, Page } from "../lib/api";
  import { pushUndo, setUndoCallback, removeUndoCallback } from "../lib/undoStack";
  import type { UndoAction } from "../lib/undoStack";

  interface Props {
    page: Page;
    compact?: boolean;
  }

  let { page, compact = false }: Props = $props();

  let blocks: Block[] = $state([]);
  let focusedBlockId: string | null = $state(null);
  let navigatingBlock = false;
  // Imperative handles to each BlockEditor, keyed by block id, for deterministic
  // cross-block Arrow Up/Down caret movement.
  let blockRefs: Record<string, { focusForNav: (x: number, edge: "top" | "bottom") => void }> = {};
  type BacklinkTreeNode = { block: Block; depth: number };
  type BacklinkView = BacklinkResult & { sourcePageTitle: string; tree: BacklinkTreeNode[] };

  let backlinks: BacklinkView[] = $state([]);
  let parentPage: Page | null = $state(null);
  let childPages: Page[] = $state([]);
  let loadError: string | null = $state(null);
  let selectedBlockIds: Set<string> = $state(new Set());
  let collapsedIds: Set<string> = $state(new Set());
  const pageLoadState = createPageLoadState();

  const BLOCK_SHELL_GAP = 2;
  const DEFAULT_BLOCK_HEIGHT = 68;
  const BLOCK_WINDOW_OVERSCAN_PX = 720;

  let blockHeights = new SvelteMap<string, number>();
  let blocksViewportEl: HTMLDivElement | null = $state(null);
  let blocksRelTop = $state(0);
  let blocksViewportHeight = $state(800);
  let windowAnchorBlockId: string | null = $state(null);

  const blockRenderState = $derived.by(() => buildBlockRenderState(blocks, collapsedIds));
  const visibleBlocks = $derived(blockRenderState.visibleBlocks);
  const virtualWindow = $derived.by(() => {
    const anchorIndex = windowAnchorBlockId
      ? blockRenderState.visibleIndexById.get(windowAnchorBlockId) ?? null
      : null;

    return computeVirtualWindow(visibleBlocks, {
      scrollTop: Math.max(0, blocksRelTop),
      viewportHeight: blocksViewportHeight,
      measuredHeights: blockHeights,
      defaultHeight: DEFAULT_BLOCK_HEIGHT,
      overscanPx: BLOCK_WINDOW_OVERSCAN_PX,
      anchorIndex,
    });
  });
  const windowedBlocks = $derived(virtualWindow.items);

  $effect(() => {
    const activeIds = new Set(blocks.map((block) => block.id));
    for (const id of Array.from(blockHeights.keys())) {
      if (!activeIds.has(id)) {
        blockHeights.delete(id);
      }
    }
  });

  $effect(() => {
    if (!blocksViewportEl) return;
    const parent = blocksViewportEl.closest(".main-content") as HTMLElement | null;
    if (!parent) return;

    const update = () => {
      const parentRect = parent.getBoundingClientRect();
      const viewportRect = blocksViewportEl!.getBoundingClientRect();
      blocksRelTop = Math.max(0, parentRect.top - viewportRect.top);
      blocksViewportHeight = parent.clientHeight;
    };

    update();
    parent.addEventListener("scroll", update, { passive: true });

    const resizeObserver = new ResizeObserver(update);
    resizeObserver.observe(parent);
    resizeObserver.observe(blocksViewportEl);

    return () => {
      parent.removeEventListener("scroll", update);
      resizeObserver.disconnect();
    };
  });

  $effect(() => {
    const pageId = page.id;
    const handleRevealBlock = (event: Event) => {
      const detail = (event as CustomEvent<{ pageId: string; blockId: string; align?: ScrollLogicalPosition }>).detail;
      if (!detail || detail.pageId !== pageId) return;
      void revealBlock(detail.blockId, detail.align ?? "center");
    };

    window.addEventListener("page-content-reveal-block", handleRevealBlock);
    return () => window.removeEventListener("page-content-reveal-block", handleRevealBlock);
  });

  function hasChildren(blockId: string): boolean {
    return (blockRenderState.childrenByParent.get(blockId)?.length ?? 0) > 0;
  }

  function isBlockVisible(blockId: string): boolean {
    return blockRenderState.visibleIds.has(blockId);
  }

  function getBlockDepth(blockId: string): number {
    return blockRenderState.depthById.get(blockId) ?? 0;
  }

  function trackBlockHeight(node: HTMLElement, blockId: string) {
    let currentBlockId = blockId;

    const update = () => {
      const nextHeight = Math.max(1, Math.ceil(node.getBoundingClientRect().height)) + BLOCK_SHELL_GAP;
      if (blockHeights.get(currentBlockId) === nextHeight) return;
      blockHeights.set(currentBlockId, nextHeight);
    };

    update();
    const resizeObserver = new ResizeObserver(update);
    resizeObserver.observe(node);

    return {
      update(nextBlockId: string) {
        currentBlockId = nextBlockId;
        update();
      },
      destroy() {
        resizeObserver.disconnect();
      },
    };
  }

  function getRenderedBlockEl(blockId: string): HTMLElement | null {
    return document.querySelector(`[data-block-id="${blockId}"]`) as HTMLElement | null;
  }

  async function ensureBlockRendered(blockId: string): Promise<boolean> {
    if (!isBlockVisible(blockId)) return false;
    if (getRenderedBlockEl(blockId)) return true;

    windowAnchorBlockId = blockId;
    await tick();
    return getRenderedBlockEl(blockId) !== null;
  }

  async function revealBlock(
    blockId: string,
    align: ScrollLogicalPosition = "nearest"
  ): Promise<boolean> {
    const rendered = await ensureBlockRendered(blockId);
    const blockEl = getRenderedBlockEl(blockId);
    if (!rendered || !blockEl) {
      if (windowAnchorBlockId === blockId) {
        windowAnchorBlockId = null;
      }
      return false;
    }

    blockEl.scrollIntoView({ block: align });
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (windowAnchorBlockId === blockId) {
          windowAnchorBlockId = null;
        }
      });
    });
    return true;
  }

  function toggleCollapse(blockId: string) {
    const newSet = new Set(collapsedIds);
    if (newSet.has(blockId)) {
      newSet.delete(blockId);
    } else {
      newSet.add(blockId);
    }
    collapsedIds = newSet;
  }

  function currentPageLoad(): PageLoadRequest {
    return capturePageLoad(pageLoadState, page.id, page.title);
  }

  // Register undo callback for THIS page (supports multiple instances in journal view)
  $effect(() => {
    if (page?.id) {
      setUndoCallback(page.id, (_action: UndoAction) => {
        void loadBlocks(currentPageLoad());
      });
      return () => {
        removeUndoCallback(page.id);
      };
    }
  });

  // Load blocks when page changes
  $effect(() => {
    const pageId = page?.id;
    const pageTitle = page?.title;
    if (pageId) {
      const request = beginPageLoad(pageLoadState, pageId, pageTitle ?? "");
      void loadBlocks(request);
      void loadBacklinks(request);
      void loadHierarchy(request);
    }
  });

  async function loadBlocks(request: PageLoadRequest = currentPageLoad()) {
    try {
      if (isCurrentPageLoad(pageLoadState, request)) {
        loadError = null;
        windowAnchorBlockId = null;
        blockHeights.clear();
      }

      const loadedBlocks = await listBlocks(request.pageId);
      if (!isCurrentPageLoad(pageLoadState, request)) return;

      let nextBlocks = loadedBlocks;
      // If no blocks exist, create an empty one
      if (nextBlocks.length === 0) {
        const newBlock = await createBlock(request.pageId, null, 0, "");
        if (!isCurrentPageLoad(pageLoadState, request)) return;
        nextBlocks = [newBlock];
      }
      blocks = nextBlocks;
    } catch (e: any) {
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      loadError = e?.toString() || "Unknown error loading blocks";
      console.error("loadBlocks failed:", e);
    }
  }

  async function loadHierarchy(request: PageLoadRequest = currentPageLoad()) {
    if (isCurrentPageLoad(pageLoadState, request)) {
      parentPage = null;
      childPages = [];
    }

    try {
      await applyIfCurrentPageLoad(
        pageLoadState,
        request,
        async () => {
          const [nextParentPage, nextChildPages] = await Promise.all([
            request.pageTitle.includes("/") ? getParentPage(request.pageTitle) : Promise.resolve(null),
            getChildPages(request.pageTitle),
          ]);
          return { nextParentPage, nextChildPages };
        },
        ({ nextParentPage, nextChildPages }) => {
          parentPage = nextParentPage;
          childPages = nextChildPages;
        }
      );
    } catch (e) {
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      console.warn("[hierarchy] Failed to load hierarchy:", e);
      parentPage = null;
      childPages = [];
    }
  }

  async function loadBacklinks(request: PageLoadRequest = currentPageLoad()) {
    if (isCurrentPageLoad(pageLoadState, request)) {
      backlinks = [];
    }

    try {
      const backlinkResults = await getBacklinks(request.pageId);
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      console.log("[telemetry] backlinks fetched", JSON.stringify({
        pageId: request.pageId,
        pageTitle: request.pageTitle,
        count: backlinkResults.length,
      }));
      const pageBlockCache = new Map<string, Block[]>();
      const pageTitleCache = new Map<string, string>();

      const renderedBacklinks = await Promise.all(backlinkResults.map(async (result) => {
        let sourceBlocks = pageBlockCache.get(result.block.page_id);
        if (!sourceBlocks) {
          sourceBlocks = await listBlocks(result.block.page_id);
          pageBlockCache.set(result.block.page_id, sourceBlocks);
        }

        let sourcePageTitle = pageTitleCache.get(result.block.page_id);
        if (!sourcePageTitle) {
          const sourcePage = await getPage({ id: result.block.page_id });
          sourcePageTitle = sourcePage.title;
          pageTitleCache.set(result.block.page_id, sourcePageTitle);
        }

        return {
          ...result,
          sourcePageTitle,
          tree: buildBacklinkTree(result.block.id, sourceBlocks),
        };
      }));
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      backlinks = renderedBacklinks;
      console.log("[telemetry] backlinks rendered", JSON.stringify({
        pageId: request.pageId,
        pageTitle: request.pageTitle,
        renderedCount: backlinks.length,
        items: backlinks.map((b) => ({
          sourcePageTitle: b.sourcePageTitle,
          rootBlockId: b.block.id,
          rootContent: b.block.content.slice(0, 80),
          treeSize: b.tree.length,
        })),
      }));
    } catch {
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      backlinks = [];
    }
  }

  function buildBacklinkTree(rootBlockId: string, sourceBlocks: Block[]): BacklinkTreeNode[] {
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

    const root = blockMap.get(rootBlockId);
    if (!root) return [];

    const tree: BacklinkTreeNode[] = [];
    const visit = (block: Block, depth: number) => {
      tree.push({ block, depth });
      const children = childrenByParent.get(block.id) ?? [];
      for (const child of children) {
        visit(child, depth + 1);
      }
    };

    visit(root, 0);
    return tree;
  }

  // Track block content before editing starts, so undo can restore it
  let preEditSnapshots: Map<string, Block> = new Map();

  function handleFocus(blockId: string) {
    focusedBlockId = blockId;
    selectedBlockIds = new Set();
    // Snapshot the block content before the user edits it
    const block = blockRenderState.blockById.get(blockId);
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

  async function handleEnter(blockId: string, content: string, _orderIndex: number, atStart: boolean) {
    try {
      const block = blocks.find((b) => b.id === blockId);
      if (!block) return;

      // Persist the current block content before any structural operation
      // (create/move), otherwise write-page operations can serialize stale empty text.
      if (block.content !== content) {
        await updateBlock(blockId, content);
        block.content = content;
        blocks = [...blocks];
      }

      // Enter at the very start of a block inserts an empty sibling above it.
      if (atStart) {
        const parentId = block.parent_id;
        const siblings = blocks
          .filter((b) => b.parent_id === parentId)
          .sort((a, b) => a.order_index - b.order_index);
        const currentSiblingIndex = siblings.findIndex((b) => b.id === blockId);
        const insertOrder = currentSiblingIndex >= 0 ? siblings[currentSiblingIndex].order_index : block.order_index;

        // Shift siblings at/after insert point down by one to keep deterministic ordering.
        for (const sibling of siblings) {
          if (sibling.id === blockId) continue;
          if (sibling.order_index >= insertOrder) {
            await moveBlock(sibling.id, sibling.parent_id, sibling.order_index + 1);
            sibling.order_index += 1;
          }
        }

        const newBlock = await createBlock(page.id, parentId, insertOrder, "");
        const idx = blocks.findIndex((b) => b.id === blockId);
        blocks = [...blocks.slice(0, idx), newBlock, ...blocks.slice(idx)];

        requestAnimationFrame(() => {
          focusedBlockId = newBlock.id;
          const el = document.querySelector(`[data-block-id="${newBlock.id}"] .block-content`);
          if (el) {
            el.scrollIntoView({ block: "nearest" });
            (el as HTMLElement).click();
          }
        });
        return;
      }

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

  function handleNavigate(blockId: string, direction: "up" | "down", caretX?: number) {
    navigatingBlock = true;
    const idx = blockRenderState.visibleIndexById.get(blockId) ?? -1;
    const targetIdx = direction === "up" ? idx - 1 : idx + 1;
    if (targetIdx >= 0 && targetIdx < visibleBlocks.length) {
      const target = visibleBlocks[targetIdx];
      // Moving up lands on the target's BOTTOM line; down lands on its TOP.
      const edge: "top" | "bottom" = direction === "up" ? "bottom" : "top";
      focusedBlockId = target.id;
      void revealBlock(target.id).then((rendered) => {
        if (!rendered) return;
        blockRefs[target.id]?.focusForNav(caretX ?? 0, edge);
      });
    }
  }

  async function handleIndent(blockId: string, direction: "in" | "out", currentContent?: string) {
    const idx = blocks.findIndex((b) => b.id === blockId);
    const block = blocks[idx];
    if (!block) return;

    // Persist latest editor text before structural move. This avoids
    // move/write operations serializing stale empty content from DB.
    if (typeof currentContent === "string" && currentContent !== block.content) {
      await persistBlockContentIfChanged(block, currentContent, (id, value) => updateBlock(id, value));
      blocks = [...blocks];
    }

    console.log("[telemetry] indent start", JSON.stringify({
      pageId: page.id,
      pageTitle: page.title,
      blockId,
      direction,
      content: block.content.slice(0, 80),
      currentContent: (currentContent ?? "").slice(0, 80),
      parentId: block.parent_id,
      orderIndex: block.order_index,
    }));

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
        console.log("[telemetry] indent in done", JSON.stringify({
          blockId: block.id,
          newParentId: block.parent_id,
          newOrderIndex: block.order_index,
          prevSiblingId: prevSibling.id,
        }));
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
        console.log("[telemetry] indent out done", JSON.stringify({
          blockId: block.id,
          newParentId: block.parent_id,
          newOrderIndex: block.order_index,
        }));
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

  function jumpToSourceBlock(sourcePageTitle: string, sourceBlockId: string) {
    window.dispatchEvent(new CustomEvent("navigate-page", {
      detail: {
        pageName: sourcePageTitle,
        sourceBlockId,
        sourcePageTitle,
        targetBlockId: sourceBlockId,
      },
    }));
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

  <div class="blocks-container" bind:this={blocksViewportEl}>
    {#if virtualWindow.topSpacer > 0}
      <div class="virtual-spacer" style={`height: ${virtualWindow.topSpacer}px;`} aria-hidden="true"></div>
    {/if}
    {#each windowedBlocks as block (block.id)}
      <div
        class="block-shell"
        id={`block-${block.id}`}
        data-block-id={block.id}
        use:trackBlockHeight={block.id}
      >
        <BlockEditor
          bind:this={blockRefs[block.id]}
          {block}
          pageId={page.id}
          pageTitle={page.title}
          depth={getBlockDepth(block.id)}
          focused={focusedBlockId === block.id}
          selected={selectedBlockIds.has(block.id)}
          hasChildren={hasChildren(block.id)}
          collapsed={collapsedIds.has(block.id)}
          onFocus={handleFocus}
          onBlur={handleBlur}
          onEnter={handleEnter}
          onDelete={handleDelete}
          onNavigate={handleNavigate}
          onIndent={handleIndent}
          onBulletClick={handleBulletClick}
          onPasteBlocks={handlePasteBlocks}
          onToggleCollapse={toggleCollapse}
        />
      </div>
    {/each}
    {#if virtualWindow.bottomSpacer > 0}
      <div class="virtual-spacer" style={`height: ${virtualWindow.bottomSpacer}px;`} aria-hidden="true"></div>
    {/if}
  </div>

  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="click-below" onclick={handleClickBelow}></div>

  {#if parentPage || childPages.length > 0}
    <div class="hierarchy-section">
      <h3 class="hierarchy-title">Hierarchy</h3>
      {#if parentPage}
        <div class="hierarchy-parents">
          <button class="hierarchy-link parent-link" type="button"
            onclick={() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: parentPage!.title }))}
          >📁 {parentPage.title}</button>
        </div>
      {/if}
      {#if childPages.length > 0}
        <div class="hierarchy-children">
          <div class="children-label">Children:</div>
          <div class="children-list">
            {#each childPages as child}
              <button class="hierarchy-link child-link" type="button"
                onclick={() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: child.title }))}
              >📄 {child.title}</button>
            {/each}
          </div>
        </div>
      {/if}
    </div>
  {/if}

  {#if backlinks.length > 0}
    <div class="backlinks-section">
      <h3 class="backlinks-title">{backlinks.length} Linked Reference{backlinks.length > 1 ? "s" : ""}</h3>
      <div class="backlinks-list">
        {#each backlinks as bl}
          <div class="backlink-item">
            <button
              class="backlink-source-page"
              type="button"
              onclick={() => jumpToSourceBlock(bl.sourcePageTitle, bl.block.id)}
              title="Open source block"
            >
              {bl.sourcePageTitle}
            </button>
            <div class="backlink-tree">
              {#each bl.tree as node}
                <button
                  class="backlink-node"
                  type="button"
                  style={`padding-left: ${node.depth * 24}px`}
                  onclick={() => jumpToSourceBlock(bl.sourcePageTitle, node.block.id)}
                  title="Jump to this block"
                >
                  <span class="backlink-bullet">•</span>
                  <div class="backlink-content" use:hydrateRenderedMedia={node.block.content}>
                    {@html renderBlock(node.block.content)}
                  </div>
                </button>
              {/each}
            </div>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .page-content {
    padding: 0;
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
  }

  .block-shell {
    padding-bottom: 2px;
    box-sizing: border-box;
  }

  .virtual-spacer {
    width: 100%;
    pointer-events: none;
  }

  .click-below {
    min-height: 72px;
    cursor: text;
  }

  .compact {
    padding: 0;
  }

  .compact .page-title {
    font-size: 16px;
    margin-bottom: 4px;
  }

  .compact .click-below {
    min-height: 0;
  }

  .hierarchy-section {
    margin-top: 18px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-secondary);
  }

  .hierarchy-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
    margin: 0 0 10px 0;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .hierarchy-parents { margin-bottom: 10px; }

  .hierarchy-children { display: flex; flex-direction: column; gap: 6px; }

  .children-label {
    font-size: 11px;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-bottom: 4px;
  }

  .children-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-left: 12px;
  }

  .hierarchy-link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: none;
    background: none;
    padding: 6px 8px;
    cursor: pointer;
    font-size: 13px;
    color: var(--text-link);
    border-radius: 4px;
    text-align: left;
  }

  .hierarchy-link:hover {
    background-color: var(--bg-hover);
    text-decoration: underline;
  }

  .parent-link { font-weight: 500; color: var(--text-primary); }
  .child-link { font-size: 12px; }

  .backlinks-section {
    margin-top: 18px;
    padding-top: 12px;
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
    padding: 10px 12px;
    background: var(--bg-secondary);
    border-radius: 6px;
    font-size: 14px;
  }

  .backlink-source-page {
    display: inline-flex;
    align-items: center;
    border: none;
    background: none;
    padding: 0;
    cursor: pointer;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
    margin-bottom: 8px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .backlink-source-page:hover {
    color: var(--text-primary);
    text-decoration: underline;
  }

  .backlink-tree {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .backlink-node {
    width: 100%;
    border: none;
    background: none;
    padding-top: 2px;
    padding-right: 0;
    padding-bottom: 2px;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    text-align: left;
    cursor: pointer;
    border-radius: 4px;
  }

  .backlink-node:hover {
    background: var(--bg-hover);
  }

  .backlink-bullet {
    color: var(--text-muted);
    line-height: 1.6;
    flex-shrink: 0;
  }

  .backlink-content {
    min-width: 0;
    color: var(--text-primary);
  }

  .backlink-content :global(.page-link),
  .backlink-content :global(.tag) {
    cursor: pointer;
  }
</style>
