<script lang="ts">
  import { highlightTerm, clearHighlights } from "../lib/highlight";
  import { SvelteMap } from "svelte/reactivity";
  import { tick } from "svelte";
  import BlockEditor from "./BlockEditor.svelte";
  import { listBlocks, createBlock, deleteBlock, updateBlock, moveBlock, getBacklinks, getPage, getParentPage, getChildPages } from "../lib/api";
  import { persistBlockContentIfChanged } from "../lib/persistence";
  import { planIndentSelection } from "../lib/blockIndent";
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
  import { aiSummarizeSelection, wrapKnownTermsInText, type TagTerm } from "../lib/knowledge";
  import { listen } from "@tauri-apps/api/event";

  interface Props {
    page: Page;
    compact?: boolean;
    /** Term to highlight on arrival, e.g. what was searched in the graph. */
    highlight?: string;
  }

  let { page, compact = false, highlight = "" }: Props = $props();

  let blocks: Block[] = $state([]);

  // Highlight after the blocks are in the DOM. Depending on `blocks` as well as
  // `highlight` matters: navigation renders the page before its content loads,
  // so running only on the prop would search an empty container and find
  // nothing.
  $effect(() => {
    const term = highlight;
    void blocks.length;
    const container = blocksViewportEl;
    if (!container) return;
    if (!term.trim()) {
      clearHighlights(container);
      return;
    }
    // One frame's delay so `{@html}` block content has been committed.
    const handle = requestAnimationFrame(() => {
      const first = highlightTerm(container, term);
      first?.scrollIntoView({ block: "center", behavior: "smooth" });
    });
    return () => cancelAnimationFrame(handle);
  });
  let focusedBlockId: string | null = $state(null);
  let navigatingBlock = false;
  // Imperative handles to each BlockEditor, keyed by block id, for deterministic
  // cross-block Arrow Up/Down caret movement.
  let blockRefs: Record<string, { focusForNav: (x: number, edge: "top" | "bottom") => void }> = {};
  type BacklinkTreeNode = { block: Block; depth: number };
  type BacklinkView = BacklinkResult & { sourcePageTitle: string; tree: BacklinkTreeNode[] };

  let backlinks: BacklinkView[] = $state([]);
  // Rendering every linked reference at once is what actually crashes the
  // app for pages with thousands of backlinks (e.g. a flashcard import
  // where ~7500 blocks all tag the same topic): each entry runs full
  // markdown rendering + a media-hydration DOM action, and doing that for
  // thousands of entries synchronously balloons the WebKit renderer's
  // memory by many GB and aborts it. Cap the initial render and let the
  // user expand incrementally, same idea as Logseq's linked-references UX.
  const BACKLINKS_PAGE_SIZE = 50;
  let backlinksRenderLimit = $state(BACKLINKS_PAGE_SIZE);
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

  // Fired by ReferencePanel's "Insert into page" action, which writes
  // directly to the DB/disk via a Tauri command rather than through this
  // component's own createBlock/updateBlock calls — so this component
  // needs an explicit signal to refresh its local `blocks` state instead
  // of going stale.
  $effect(() => {
    const pageId = page.id;
    const handleReloadBlocks = (event: Event) => {
      const detail = (event as CustomEvent<{ pageId: string }>).detail;
      if (!detail || detail.pageId !== pageId) return;
      void listBlocks(page.id).then((updated) => {
        blocks = updated;
      });
    };

    window.addEventListener("page-content-reload-blocks", handleReloadBlocks);
    return () => window.removeEventListener("page-content-reload-blocks", handleReloadBlocks);
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

  // Per-source-page index used by buildBacklinkTree, keyed by page_id.
  // Building the blockMap/childrenByParent grouping is O(n) in the size of
  // that source page's block list. A page can be referenced by MANY blocks
  // within the SAME source page (e.g. a large flashcard import where every
  // block tags the same topic), so `backlinkResults` can have thousands of
  // entries that all share one `sourceBlocks` array. Previously this index
  // was rebuilt from scratch on every single backlink result, which made
  // rendering backlinks for such a page O(n^2) in the number of blocks
  // (thousands of backlinks x thousands of blocks each = tens of millions
  // of Map/array operations on the main thread) -- this froze and eventually
  // crashed the app for large pages. Building the index once per source
  // page and reusing it for every backlink result makes this O(n) overall.
  type BacklinkSourceIndex = {
    blockMap: Map<string, Block>;
    childrenByParent: Map<string | null, Block[]>;
  };

  function buildBacklinkSourceIndex(sourceBlocks: Block[]): BacklinkSourceIndex {
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

    return { blockMap, childrenByParent };
  }

  async function loadBacklinks(request: PageLoadRequest = currentPageLoad()) {
    if (isCurrentPageLoad(pageLoadState, request)) {
      backlinks = [];
      backlinksRenderLimit = BACKLINKS_PAGE_SIZE;
    }

    try {
      const backlinkResults = await getBacklinks(request.pageId);
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      console.log(
        `[backlinks] page=${request.pageTitle} fetched ${backlinkResults.length} backlink(s)`
      );

      // Resolve per-source-page data (blocks, title, tree index) exactly
      // once per unique page_id, not once per backlink result. A single
      // source page can appear in `backlinkResults` thousands of times
      // (e.g. every block on a page tags the same topic), and since
      // `Array.map(async ...)` starts every callback synchronously up to
      // its first `await`, a naive "check cache, then await + populate"
      // pattern lets every one of those callbacks see an empty cache
      // before any of them has finished populating it -- causing the
      // exact same `listBlocks`/`getPage` IPC call to fire thousands of
      // times in a race. Storing the in-flight Promise itself (computed
      // synchronously, before any await) closes that race: concurrent
      // lookups for the same page_id all await the same one promise.
      const uniquePageIds = Array.from(new Set(backlinkResults.map((r) => r.block.page_id)));
      const blocksPromiseCache = new Map<string, Promise<Block[]>>();
      const titlePromiseCache = new Map<string, Promise<string>>();
      for (const pageId of uniquePageIds) {
        blocksPromiseCache.set(pageId, listBlocks(pageId));
        titlePromiseCache.set(pageId, getPage({ id: pageId }).then((p) => p.title));
      }

      const indexCache = new Map<string, Promise<BacklinkSourceIndex>>();
      for (const pageId of uniquePageIds) {
        indexCache.set(
          pageId,
          blocksPromiseCache.get(pageId)!.then((sourceBlocks) => buildBacklinkSourceIndex(sourceBlocks))
        );
      }

      const renderedBacklinks = await Promise.all(backlinkResults.map(async (result) => {
        const [sourcePageTitle, index] = await Promise.all([
          titlePromiseCache.get(result.block.page_id)!,
          indexCache.get(result.block.page_id)!,
        ]);

        return {
          ...result,
          sourcePageTitle,
          tree: buildBacklinkTree(result.block.id, index),
        };
      }));
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      backlinks = renderedBacklinks;
      console.log(
        `[backlinks] page=${request.pageTitle} rendered ${backlinks.length} backlink(s)`
      );
    } catch {
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      backlinks = [];
    }
  }

  function buildBacklinkTree(rootBlockId: string, index: BacklinkSourceIndex): BacklinkTreeNode[] {
    const { blockMap, childrenByParent } = index;
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

  /**
   * Indent or outdent the whole multi-block selection together, preserving
   * relative structure. No-ops silently for units that can't move (e.g. the
   * first child of the document on indent). Selection is preserved.
   */
  async function handleIndentSelection(direction: "in" | "out") {
    if (selectedBlockIds.size === 0) return;
    const plan = planIndentSelection(blocks, selectedBlockIds, direction);
    if (plan.moves.length === 0) return; // nothing movable — silent no-op

    const keep = new Set(selectedBlockIds);
    try {
      for (const move of plan.moves) {
        await moveBlock(move.id, move.newParentId, move.newOrderIndex);
      }
      blocks = plan.blocks;
      selectedBlockIds = keep;
    } catch (e) {
      console.error("Failed to indent/outdent selection:", e);
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
    // Clear any active editor focus AND actively blur the DOM so subsequent
    // keydowns (Tab, etc.) reach the window handler instead of a stale
    // CodeMirror editor that our `focusedBlockId` reset alone doesn't
    // physically defocus.
    if (focusedBlockId) {
      focusedBlockId = null;
    }
    const active = document.activeElement as HTMLElement | null;
    if (active && (active.isContentEditable || active.closest(".cm-editor"))) {
      active.blur();
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

  let analyzingSelection = $state(false);
  let analyzeSelectionError = $state("");
  let analyzeSelectionProgress = $state("");

  /// Summarizes the selected blocks' content, in-place wraps the AI's
  /// identified key terms as `[[wiki-link]]`s wherever they verbatim occur
  /// in the selected blocks' own text (so tagging is an actual edit to
  /// the page, not just a label in the summary), and inserts a clean
  /// title-answer + one heading/paragraph per topic as a new block right
  /// after the last selected block — the same per-topic summary shape
  /// used by "Research this page" and media imports (so a selection
  /// covering several distinct subjects gets a paragraph per subject
  /// instead of one blended summary), just applied to a manual
  /// text/block selection instead of a whole page.
  async function handleAnalyzeSelected() {
    if (selectedBlockIds.size === 0 || analyzingSelection) return;
    // Document order, not click order, so the summary reads coherently
    // regardless of which block the user shift-clicked from.
    const selected = blocks.filter((b) => selectedBlockIds.has(b.id));
    const text = selected.map((b) => b.content).join("\n\n").trim();
    if (!text) return;

    analyzingSelection = true;
    analyzeSelectionError = "";
    analyzeSelectionProgress = "Analyzing selection...";
    const unlisten = await listen<string>("ai-selection-summary-progress", (e) => {
      analyzeSelectionProgress = e.payload;
    });
    try {
      const summary = await aiSummarizeSelection(text, page.title);
      const allTags: TagTerm[] = [];
      const seenTags = new Set<string>();
      for (const topic of summary.topics) {
        for (const tag of topic.tags ?? []) {
          const key = tag.term.trim().toLowerCase();
          if (key && !seenTags.has(key)) {
            seenTags.add(key);
            allTags.push(tag);
          }
        }
      }

      if (allTags.length) {
        analyzeSelectionProgress = "Linking key terms...";
        for (const block of selected) {
          const wrapped = await wrapKnownTermsInText(block.content, allTags);
          if (wrapped !== block.content) {
            await updateBlock(block.id, wrapped);
            blocks = blocks.map((b) => (b.id === block.id ? { ...b, content: wrapped } : b));
          }
        }
      }

      const lastBlock = selected[selected.length - 1];
      const siblings = blocks.filter((b) => b.parent_id === lastBlock.parent_id);
      const siblingIdx = siblings.findIndex((b) => b.id === lastBlock.id);
      const newOrder = siblingIdx + 1;

      // Build a block tree, not one block of flat text. Grafium is an
      // outliner: a heading only "owns" the prose beneath it when that prose
      // is its child, so emitting headings and paragraphs as siblings leaves
      // every topic structurally disconnected from its own summary.
      const rootContent = summary.title_answer
        ? `**${summary.title_answer}**`
        : "**Summary**";
      const rootBlock = await createBlock(page.id, lastBlock.parent_id, newOrder, rootContent);
      const created: Block[] = [rootBlock];

      for (const [index, topic] of summary.topics.entries()) {
        const heading = await createBlock(
          page.id,
          rootBlock.id,
          index,
          `### ${topic.topic.trim()}`,
        );
        created.push(heading);
        const body = topic.summary.trim();
        if (body) {
          created.push(await createBlock(page.id, heading.id, 0, body));
        }
      }

      const insertAt = blocks.findIndex((b) => b.id === lastBlock.id);
      blocks = [...blocks.slice(0, insertAt + 1), ...created, ...blocks.slice(insertAt + 1)];
      selectedBlockIds = new Set();
    } catch (e) {
      analyzeSelectionError = e instanceof Error ? e.message : String(e);
    } finally {
      unlisten();
      analyzingSelection = false;
      analyzeSelectionProgress = "";
    }
  }

  /**
   * Resolves the block a DOM node lives in, via the `data-block-id` marker on
   * each rendered block shell.
   */
  function blockIdFromNode(node: Node | null): string | null {
    if (!node) return null;
    const el = node instanceof Element ? node : node.parentElement;
    const shell = el?.closest?.("[data-block-id]") as HTMLElement | null;
    return shell?.dataset?.blockId ?? null;
  }

  /**
   * Promotes a native text selection that spans multiple blocks into a
   * block-level selection, the way Logseq does.
   *
   * Dragging across block boundaries is the primary way users select several
   * blocks, but a DOM range carries no block semantics, so structural commands
   * (Tab/Shift+Tab to indent, Backspace to delete, Analyze Selection) had
   * nothing to act on and Tab fell through to native focus traversal — which
   * looked like "selecting blocks then pressing Tab just clears them".
   *
   * A drag inside a single block is left alone so partial-text selection (copy,
   * Analyze Selection on a phrase) keeps working.
   */
  function promoteTextSelectionToBlocks() {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;

    const startId = blockIdFromNode(sel.anchorNode);
    const endId = blockIdFromNode(sel.focusNode);
    if (!startId || !endId || startId === endId) return;

    const startIdx = blocks.findIndex((b) => b.id === startId);
    const endIdx = blocks.findIndex((b) => b.id === endId);
    // Both ends must belong to *this* PageContent instance; the journal renders
    // one instance per day, so a cross-day drag simply isn't a block selection.
    if (startIdx === -1 || endIdx === -1) return;

    const [from, to] = startIdx < endIdx ? [startIdx, endIdx] : [endIdx, startIdx];
    const next = new Set<string>();
    for (let i = from; i <= to; i++) {
      next.add(blocks[i].id);
    }

    selectedBlockIds = next;
    focusedBlockId = null;
    sel.removeAllRanges();
    const active = document.activeElement as HTMLElement | null;
    if (active && (active.isContentEditable || active.closest(".cm-editor"))) {
      active.blur();
    }
  }

  function handleSelectionMouseUp() {
    // Defer so the browser has committed the final range for this drag.
    setTimeout(promoteTextSelectionToBlocks, 0);
  }

  /**
   * True for both Tab and Shift+Tab.
   *
   * WebKitGTK reports Shift+Tab as the X11 `ISO_Left_Tab` keysym rather than
   * `Tab`, so matching only `e.key === "Tab"` silently loses every outdent.
   * `e.code` is layout-independent and stays `"Tab"` for both, with the key
   * names kept as a fallback for engines that don't populate `code`.
   */
  function isTabKey(e: KeyboardEvent): boolean {
    return e.code === "Tab" || e.key === "Tab" || e.key === "ISO_Left_Tab";
  }

  function handleKeydownForSelection(e: KeyboardEvent) {
    if (selectedBlockIds.size === 0) return;
    if (isTabKey(e)) {
      // Selection presence is the intent signal — no need to consult
      // document.activeElement. Multi-block Tab always takes precedence over
      // in-editor Tab (a stale editor focus from the last click would otherwise
      // let the browser move focus and clear the selection).
      e.preventDefault();
      void handleIndentSelection(e.shiftKey ? "out" : "in");
    } else if (e.key === "Backspace" || e.key === "Delete") {
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

<svelte:window onkeydown={handleKeydownForSelection} onmouseup={handleSelectionMouseUp} />

<div class="page-content" class:compact>
  <h1 class="page-title">{page.title}</h1>

  {#if loadError}
    <div class="load-error" style="color: #f38ba8; background: #1e1e2e; padding: 12px; border-radius: 8px; margin-bottom: 16px; font-family: monospace; font-size: 13px; white-space: pre-wrap;">
      Error: {loadError}
    </div>
  {/if}

  {#if selectedBlockIds.size > 0}
    <div class="selection-toolbar">
      <span class="selection-count">{selectedBlockIds.size} selected</span>
      <button
        class="selection-toolbar-btn"
        onclick={handleAnalyzeSelected}
        disabled={analyzingSelection}
      >
        {analyzingSelection ? (analyzeSelectionProgress || "Analyzing…") : "Analyze Selected"}
      </button>
      <button class="selection-toolbar-btn danger" onclick={handleDeleteSelected} disabled={analyzingSelection}>
        Delete
      </button>
      <button class="selection-toolbar-btn" onclick={() => { selectedBlockIds = new Set(); }} disabled={analyzingSelection}>
        Clear
      </button>
    </div>
    {#if analyzeSelectionError}
      <div class="selection-toolbar-error">{analyzeSelectionError}</div>
    {/if}
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
        {#each backlinks.slice(0, backlinksRenderLimit) as bl}
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
        {#if backlinks.length > backlinksRenderLimit}
          <button
            class="backlinks-show-more"
            type="button"
            onclick={() => { backlinksRenderLimit += BACKLINKS_PAGE_SIZE; }}
          >
            Show {Math.min(BACKLINKS_PAGE_SIZE, backlinks.length - backlinksRenderLimit)} more (of {backlinks.length - backlinksRenderLimit} remaining)
          </button>
        {/if}
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

  /* Highlights are injected into rendered block HTML, so the selector has to
     be :global — and the colour comes from the theme's accent set rather than
     a fixed yellow, which is invisible on the amber themes and illegible on
     the light ones. */
  .blocks-container :global(mark.search-highlight) {
    background: color-mix(in srgb, var(--accent-yellow) 32%, transparent);
    color: inherit;
    border-radius: 2px;
    padding: 0 1px;
    /* Not colour alone: an underline keeps the match findable under any
       colour-vision deficiency and on a theme where the accent is subtle. */
    box-shadow: inset 0 -2px 0 var(--accent-yellow);
  }

  .blocks-container {
    display: flex;
    flex-direction: column;
  }

  .selection-toolbar {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--accent-color, #7c3aed);
    border-radius: 8px;
    padding: 8px 10px;
    margin-bottom: 8px;
  }

  .selection-count {
    font-size: 12px;
    color: var(--text-secondary, #aaa);
    margin-right: 4px;
  }

  .selection-toolbar-btn {
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-secondary, #1a1a24);
    color: var(--text-primary);
    cursor: pointer;
  }

  .selection-toolbar-btn:hover:not(:disabled) {
    border-color: var(--accent-color, #7c3aed);
  }

  .selection-toolbar-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .selection-toolbar-btn.danger {
    color: var(--error-color, #e57373);
  }

  .selection-toolbar-error {
    font-size: 12px;
    color: var(--error-color, #e57373);
    margin-bottom: 8px;
  }

  .block-shell {
    padding-bottom: 2px;
    box-sizing: border-box;
    /*
     * Scope layout and style invalidation to the individual block.
     *
     * WebKitGTK can't use the DMABUF renderer on NVIDIA + Wayland (it aborts
     * with "Error 71 (Protocol error)"), so frames are rasterized on the CPU
     * and repaint cost scales with the invalidated area. Without containment,
     * editing one block lets WebKit treat the whole block list as dirty.
     * `paint` is deliberately omitted: it would clip CodeMirror's completion
     * tooltips, which render inside the block's own DOM subtree.
     */
    contain: layout style;
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
    background: var(--bg-secondary);
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

  .backlinks-show-more {
    width: 100%;
    margin-top: 8px;
    padding: 8px 12px;
    border: 1px solid var(--border-color, #444);
    border-radius: 4px;
    background: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 13px;
  }

  .backlinks-show-more:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }
</style>
