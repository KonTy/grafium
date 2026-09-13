<script lang="ts">
  import { highlightTerm, clearHighlights } from "../lib/highlight";
  import { SvelteMap } from "svelte/reactivity";
  import { onDestroy, tick } from "svelte";
  import BlockEditor from "./BlockEditor.svelte";
  import UnifiedPageEditor from "./UnifiedPageEditor.svelte";
  import CollectionMembers from "./CollectionMembers.svelte";
  import PageMenu from "./PageMenu.svelte";
  import {
    listBlocks,
    createBlock,
    createBlocks,
    deleteBlocks,
    updateBlock,
    moveBlock,
    reorderBlocks,
    getBacklinks,
    getPage,
    renamePage,
    deletePage,
    getParentPage,
    getChildPages,
    discoverLinkCandidates,
    listLinkCandidates,
    acceptLinkCandidate,
    dismissLinkCandidate,
    restoreLinkCandidate,
    undoLinkCandidateAccept,
    type LinkCandidate,
  } from "../lib/api";
  import { planIndentSelection } from "../lib/blockIndent";
  import {
    buildBlockRenderState,
    buildBulletThreadRoles,
    computeVirtualWindow,
    getAncestorGuides,
    NO_THREAD,
    nextProgressiveRenderLimit,
  } from "../lib/pageContentVirtualization";
  import { renderBlock, assetBaseDirFor, markdownHeadingSlug } from "../lib/markdown";
  import { wrapPageLinkText } from "../lib/editorFormat";
  import { bulletToTodoContent, isTaskContent, taskToBulletContent } from "../lib/taskSyntax";
  import { formatBlocksAsOutlineMarkdown, formatBlocksAsPlainText } from "../lib/blockClipboard";
  import { bionicReader } from "../lib/bionicReader";
  import { hydrateRenderedMedia } from "../lib/renderedMedia";
  import {
    applyIfCurrentPageLoad,
    beginPageLoad,
    capturePageLoad,
    createPageLoadState,
    isCurrentPageLoad,
    type PageLoadRequest,
  } from "../lib/pageContentLoad";
  import type { BacklinkResult, Block, CreateBlockBatchItem, Page } from "../lib/api";
  import { pushUndo, removeUndoActions, setUndoCallback, removeUndoCallback } from "../lib/undoStack";
  import type { BlockContentChange, UndoAction } from "../lib/undoStack";
  import { contextMenuPositionFromEvent } from "../lib/contextMenu";
  import { jobs, isTerminal } from "../lib/jobs.svelte";
  import {
    aiCreateConceptEdges,
    aiSummarizeSelection,
    wrapKnownTermsInText,
    type TagTerm,
  } from "../lib/knowledge";
  import {
    collectionMembersFromBlocks,
    getCollectionKind,
    pagesListCollections,
    pageTreeReferencesChanged,
    pageSetCollection,
    withMissingCommandFallback,
  } from "../lib/pageTree";
  import { setCurrentBlockAnchor } from "../lib/currentBlockAnchor";
  import { showToast } from "../lib/toast.svelte";
  import {
    buildBacklinkSourceIndex,
    buildBacklinkTree,
    type BacklinkSourceIndex,
    type BacklinkTreeNode,
  } from "../lib/backlinkTree";
  import { listen } from "@tauri-apps/api/event";
  import {
    clearPendingEditPageEnd,
    EDIT_PAGE_END_EVENT,
    peekPendingEditPageEnd,
    type EditPageEndDetail,
  } from "../lib/editorInsert";

  interface Props {
    page: Page;
    compact?: boolean;
    /** Term to highlight on arrival, e.g. what was searched in the graph. */
    highlight?: string;
    /** Draw the vertical lines that connect a nested block to its ancestors. */
    showBlockGuides?: boolean;
    onPageRenamed?: (page: Page) => void;
    onPageDeleted?: (parentTitle: string | null) => void;
    onLoadSettled?: () => void;
  }

  let { page, compact = false, highlight = "", showBlockGuides = true, onPageRenamed, onPageDeleted, onLoadSettled }: Props = $props();

  // Asset references in this page's blocks are resolved relative to the
  // directory its markdown file lives in, so media stored beside a page (and a
  // whole book folder copied elsewhere) keeps working. Set before any block
  // renders; cleared to the graph root when the page has no file yet.
  // Derived, not an effect: effects run after the template has already
  // rendered, so an effect would hand the *previous* page's directory to this
  // page's first render — and the journal view mounts several pages at once,
  // which no single shared value can describe.
  // A page that exists only because something linked to it has no markdown
  // file until its first block is created, which happens right below on open.
  // The `page` prop still says `null` at that point, so the freshly created
  // path is tracked here — otherwise media pasted into a brand-new page
  // resolves against the graph root and renders broken until you navigate away
  // and back.
  let materializedFilePath = $state<string | null>(null);
  let assetBaseDir = $derived(assetBaseDirFor(page.file_path ?? materializedFilePath));

  let blocks: Block[] = $state([]);
  let destroyed = false;

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
  /// Last current block for bullet threading. Survives the click-to-edit blur
  /// that clears `focusedBlockId` after ~120ms.
  let threadBlockId: string | null = $state(null);
  function emitFocusChanged(blockId: string | null) {
    setCurrentBlockAnchor(page.id, blockId);
  }
  function handleBlockAnchor(blockId: string) {
    threadBlockId = blockId;
    emitFocusChanged(blockId);
  }
  onDestroy(() => {
    destroyed = true;
    emitFocusChanged(null);
    if (linkCandidateRevealTimer !== undefined) {
      window.clearTimeout(linkCandidateRevealTimer);
    }
  });
  let navigatingBlock = false;
  // Imperative handles to each BlockEditor, keyed by block id, for deterministic
  // cross-block Arrow Up/Down caret movement.
  type BlockEditorHandle = {
    focusForNav: (x: number, edge: "top" | "bottom") => void;
    focusAtEnd: () => void;
    insertText: (text: string) => void;
  };
  let blockRefs: Record<string, BlockEditorHandle> = {};

  function pageMatchesEditRequest(detail: EditPageEndDetail): boolean {
    if (detail.pageId && detail.pageId === page.id) return true;
    if (detail.pageTitle && detail.pageTitle === page.title) return true;
    return false;
  }

  async function focusLastBlockAtEnd(insert?: string, attempt = 0) {
    const last = visibleBlocks.at(-1) ?? blocks.at(-1);
    if (!last) {
      if (attempt < 40) requestAnimationFrame(() => void focusLastBlockAtEnd(insert, attempt + 1));
      return;
    }
    document.getElementById(`block-${last.id}`)?.scrollIntoView({ block: "nearest" });
    await tick();
    const ref = blockRefs[last.id];
    if (!ref) {
      if (attempt < 40) requestAnimationFrame(() => void focusLastBlockAtEnd(insert, attempt + 1));
      return;
    }
    if (insert) ref.insertText(insert);
    else ref.focusAtEnd();
  }

  $effect(() => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<EditPageEndDetail>).detail;
      if (!detail || !pageMatchesEditRequest(detail)) return;
      if (blocks.length === 0) return;
      clearPendingEditPageEnd();
      void focusLastBlockAtEnd(detail.insert);
    };
    window.addEventListener(EDIT_PAGE_END_EVENT, handler);
    return () => window.removeEventListener(EDIT_PAGE_END_EVENT, handler);
  });

  $effect(() => {
    void blocks.length;
    void page.id;
    void page.title;
    const pending = peekPendingEditPageEnd(page);
    if (!pending || blocks.length === 0) return;
    clearPendingEditPageEnd();
    void focusLastBlockAtEnd(pending.insert);
  });
  type BacklinkView = BacklinkResult & {
    sourcePageTitle: string;
    /** Where the *source* page lives, so its media resolves against its own
     *  folder rather than the folder of whatever page you happen to be on. */
    sourceAssetBaseDir: string;
    tree: BacklinkTreeNode[];
  };

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
  const LINK_CANDIDATE_REVIEW_LIMIT = 1000;
  let linkCandidates: LinkCandidate[] = $state([]);
  let linkCandidatesLoading = $state(false);
  let linkCandidatesError = $state("");
  interface LinkCandidateGroup {
    key: string;
    candidates: LinkCandidate[];
    primary: LinkCandidate;
    occurrenceCount: number;
    anchorTexts: string[];
    sources: string[];
    canFixSpelling: boolean;
    hasCanonicalTarget: boolean;
  }
  interface LinkCandidateContextPreview {
    candidate: LinkCandidate;
    before: string;
    anchor: string;
    after: string;
    leadingEllipsis: boolean;
    trailingEllipsis: boolean;
    blockLabel: string;
  }
  type LinkCandidateUndo =
    | { kind: "accepted"; candidates: LinkCandidate[] }
    | { kind: "dismissed"; candidates: LinkCandidate[] };
  let lastLinkCandidateAction: LinkCandidateUndo | null = $state(null);
  let linkCandidatesRevealed = $state(false);
  const linkCandidateGroups = $derived.by(() => groupLinkCandidates(linkCandidates));
  const linkCandidateOccurrenceTotal = $derived.by(() =>
    linkCandidateGroups.reduce((total, group) => total + group.occurrenceCount, 0)
  );
  let parentPage: Page | null = $state(null);
  let childPages: Page[] = $state([]);
  let collectionKind: string | null = $state(null);
  let collectionMemberCount: number | undefined = $state(undefined);
  let collectionStatus: "page" | "collection" | "loading" | "unavailable" | "error" = $state("loading");
  let collectionBusy = $state(false);
  let collectionRequest = 0;
  let loadError: string | null = $state(null);
  let selectedBlockIds: Set<string> = $state(new Set());
  type SelectionMakeLinkAction = {
    blockId: string;
    from: number | null;
    to: number | null;
    text: string;
  };
  const SELECTION_MAKE_LINK_CACHE_MS = 30_000;
  let lastSelectionMakeLinkAction: (SelectionMakeLinkAction & { capturedAt: number }) | null = $state(null);
  let selectionMenu: {
    x: number;
    y: number;
    blockIds: string[];
    blockCount: number;
    canTurnTasksToBullets: boolean;
    canTurnBulletsToTodos: boolean;
    makeLink?: SelectionMakeLinkAction;
    showPageActions: boolean;
  } | null = $state(null);
  let selectionCopyMessage = $state("");
  let selectionCopyTimer: number | undefined;
  let promotedSelectionCopyText: string | null = $state(null);
  let blockSelectionDrag: {
    pointerId: number;
    startBlockId: string;
    started: boolean;
    startX: number;
    startY: number;
  } | null = null;
  let revealedBlockId: string | null = $state(null);
  let revealedBlockTimer: number | undefined;
  let linkCandidateRevealTimer: number | undefined;
  let collapsedIds: Set<string> = $state(new Set());
  const pageLoadState = createPageLoadState();
  const UNIFIED_EDITOR_PROTOTYPE_KEY = "grafium.experimental.unifiedPageEditor";
  let useUnifiedEditorPrototype = $state(false);

  const BLOCK_SHELL_GAP = 2;
  const DEFAULT_BLOCK_HEIGHT = 68;
  const BLOCK_WINDOW_OVERSCAN_PX = 720;
  const BOOK_INITIAL_RENDER_COUNT = 32;
  const BOOK_RENDER_BATCH_COUNT = 48;
  const BOOK_LOAD_AHEAD_PX = 800;

  let blockHeights = new SvelteMap<string, number>();
  let pageContentEl: HTMLDivElement | null = $state(null);
  let blocksViewportEl: HTMLDivElement | null = $state(null);
  let blocksRelTop = $state(0);
  let blocksViewportHeight = $state(800);
  let windowAnchorBlockId: string | null = $state(null);
  let progressiveBookRenderLimit = $state(BOOK_INITIAL_RENDER_COUNT);
  const isImportedBookPage = $derived.by(() => {
    const filePath = (page.file_path ?? materializedFilePath ?? "").replace(/\\/g, "/");
    return page.title.startsWith("Books/") || filePath.startsWith("pages/Books/");
  });
  const displayPageTitle = $derived(
    isImportedBookPage
      ? page.title.replace(/^Books\//, "")
      : page.is_journal
        ? page.title.replace(/_/g, "-")
        : page.title
  );
  const canRenamePage = $derived(!compact && !page.is_journal);
  const canDeletePage = $derived(canRenamePage);
  let renamingTitle = $state(false);
  let renameDraft = $state("");
  let renameError = $state("");
  let renameBusy = $state(false);
  let renameInputEl: HTMLInputElement | null = $state(null);

  function startRename() {
    renameDraft = page.title;
    renameError = "";
    renamingTitle = true;
    void tick().then(() => renameInputEl?.select());
  }

  function cancelRename() {
    renamingTitle = false;
    renameError = "";
    renameBusy = false;
  }

  async function commitRename() {
    const next = renameDraft.trim();
    if (!next || next === page.title) {
      cancelRename();
      return;
    }
    renameBusy = true;
    renameError = "";
    try {
      const existing = await getPage({ title: next }).catch(() => null);
      if (existing && existing.id !== page.id) {
        const ok = window.confirm(
          `A page named "${existing.title}" already exists.\n\nMerge this page into it? Notes from both pages will be kept.`,
        );
        if (!ok) return;
      }
      const updated = await renamePage(page.id, next);
      renamingTitle = false;
      window.dispatchEvent(new CustomEvent("page-tree-refresh"));
      if (updated.id !== page.id) {
        showToast(
          `Merged into ${updated.title}. Notes from both pages were kept.`,
          "success",
        );
      }
      onPageRenamed?.(updated);
    } catch (e: unknown) {
      renameError = e instanceof Error ? e.message : String(e);
    } finally {
      renameBusy = false;
    }
  }

  function handleRenameKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      void commitRename();
    } else if (event.key === "Escape") {
      event.preventDefault();
      cancelRename();
    }
  }

  async function deleteCurrentPage() {
    selectionMenu = null;
    if (!canDeletePage) return;
    let childCount = 0;
    try {
      childCount = (await getChildPages(page.title)).length;
    } catch {
      childCount = 0;
    }
    const extra = childCount > 0
      ? ` and ${childCount} subpage${childCount === 1 ? "" : "s"}`
      : "";
    const ok = window.confirm(
      `Delete page '${page.title}'${extra}? This removes notes, markdown files, and media that nothing else uses. This cannot be undone.`,
    );
    if (!ok) return;
    try {
      const result = await deletePage(page.id);
      window.dispatchEvent(new CustomEvent("page-tree-refresh"));
      const pages = result.deleted_pages;
      const assets = result.deleted_assets;
      showToast(
        `Deleted ${pages} page${pages === 1 ? "" : "s"}`
          + (assets > 0 ? ` and ${assets} media file${assets === 1 ? "" : "s"}` : "")
          + ".",
        "success",
      );
      onPageDeleted?.(parentPage?.title ?? null);
    } catch (e: unknown) {
      showToast(e instanceof Error ? e.message : String(e), "error");
    }
  }
  const blockSelectionOwnerId = `page-content-${Math.random().toString(36).slice(2)}`;

  function claimBlockSelectionOwner() {
    (globalThis as any).__grafiumBlockSelectionOwner = blockSelectionOwnerId;
  }

  function releaseBlockSelectionOwner() {
    if ((globalThis as any).__grafiumBlockSelectionOwner === blockSelectionOwnerId) {
      delete (globalThis as any).__grafiumBlockSelectionOwner;
    }
  }

  function isBlockSelectionOwner(): boolean {
    return (globalThis as any).__grafiumBlockSelectionOwner === blockSelectionOwnerId;
  }

  function clearBlockSelection() {
    selectedBlockIds = new Set();
    promotedSelectionCopyText = null;
    selectionMenu = null;
    lastSelectionMakeLinkAction = null;
    releaseBlockSelectionOwner();
  }

  function editableTargetOutsideThisPage(event: Event): boolean {
    const target = event.target as Element | null;
    const editable = target?.closest?.("input, textarea, select, [contenteditable='true'], .cm-editor");
    return !!editable && !pageContentEl?.contains(editable);
  }

  function nativeSelectionOutsideThisPage(): boolean {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) return false;
    const nodes = [selection.anchorNode, selection.focusNode].filter(Boolean) as Node[];
    return nodes.some((node) => !pageContentEl?.contains(node));
  }

  function canHandleBlockSelectionEvent(event: Event): boolean {
    return selectedBlockIds.size > 0
      && isBlockSelectionOwner()
      && !editableTargetOutsideThisPage(event)
      && !nativeSelectionOutsideThisPage();
  }

  const blockRenderState = $derived.by(() => buildBlockRenderState(blocks, collapsedIds));
  const visibleBlocks = $derived(blockRenderState.visibleBlocks);
  const shouldVirtualizeBlocks = $derived(!isImportedBookPage && visibleBlocks.length > 500);
  const shouldProgressivelyRenderBook = $derived(
    isImportedBookPage && visibleBlocks.length > BOOK_INITIAL_RENDER_COUNT
  );
  const visibleRenderedBlockCount = $derived(
    shouldProgressivelyRenderBook
      ? Math.min(visibleBlocks.length, progressiveBookRenderLimit)
      : visibleBlocks.length
  );
  const renderedAllVisibleBlocks = $derived(visibleRenderedBlockCount >= visibleBlocks.length);
  const showBelowPageSections = $derived(!shouldProgressivelyRenderBook || renderedAllVisibleBlocks);
  const showHierarchySection = $derived(!isImportedBookPage && showBelowPageSections);
  const collectionMembers = $derived(collectionMembersFromBlocks(blocks));
  const virtualWindow = $derived.by(() => {
    if (shouldProgressivelyRenderBook) {
      return {
        startIndex: 0,
        endIndex: visibleRenderedBlockCount,
        topSpacer: 0,
        bottomSpacer: 0,
        totalHeight: 0,
        items: visibleBlocks.slice(0, visibleRenderedBlockCount),
      };
    }

    if (!shouldVirtualizeBlocks) {
      return {
        startIndex: 0,
        endIndex: visibleBlocks.length,
        topSpacer: 0,
        bottomSpacer: 0,
        totalHeight: 0,
        items: visibleBlocks,
      };
    }

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

  function unifiedEditorPrototypeKey(pageId: string): string {
    return `${UNIFIED_EDITOR_PROTOTYPE_KEY}:${pageId}`;
  }

  let unifiedEditorPrototypePageId: string | null = null;

  $effect(() => {
    const pageId = page.id;
    if (unifiedEditorPrototypePageId === pageId) return;
    unifiedEditorPrototypePageId = pageId;
    try {
      useUnifiedEditorPrototype = localStorage.getItem(unifiedEditorPrototypeKey(pageId)) === "1";
    } catch {
      useUnifiedEditorPrototype = false;
    }
  });

  onDestroy(() => {
    if (revealedBlockTimer !== undefined) {
      window.clearTimeout(revealedBlockTimer);
    }
  });

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

    const updateLayout = () => {
      const parentRect = parent.getBoundingClientRect();
      const viewportRect = blocksViewportEl!.getBoundingClientRect();
      blocksRelTop = Math.max(0, parentRect.top - viewportRect.top);
      blocksViewportHeight = parent.clientHeight;
    };

    const handleScroll = () => {
      updateLayout();
      maybeGrowProgressiveBookRenderWindow(parent);
    };

    updateLayout();
    parent.addEventListener("scroll", handleScroll, { passive: true });

    const resizeObserver = new ResizeObserver(updateLayout);
    resizeObserver.observe(parent);
    resizeObserver.observe(blocksViewportEl);

    return () => {
      parent.removeEventListener("scroll", handleScroll);
      resizeObserver.disconnect();
    };
  });

  $effect(() => {
    const pageId = page.id;
    const handleRevealBlock = (event: Event) => {
      const detail = (event as CustomEvent<{ pageId: string; blockId: string; align?: ScrollLogicalPosition; select?: boolean }>).detail;
      if (!detail || detail.pageId !== pageId) return;
      void revealBlock(detail.blockId, detail.align ?? "center").then((revealed) => {
        if (revealed && detail.select !== false) {
          selectRevealedBlock(detail.blockId);
        }
      });
    };

    window.addEventListener("page-content-reveal-block", handleRevealBlock);
    return () => window.removeEventListener("page-content-reveal-block", handleRevealBlock);
  });

  $effect(() => {
    const pageId = page.id;
    const handleRevealFragment = (event: Event) => {
      const detail = (event as CustomEvent<{ pageId: string; fragment: string | string[] }>).detail;
      if (!detail || detail.pageId !== pageId) return;
      const fragments = Array.isArray(detail.fragment) ? detail.fragment : [detail.fragment];
      void revealHeadingFragments(fragments);
    };

    window.addEventListener("page-content-reveal-fragment", handleRevealFragment);
    return () => window.removeEventListener("page-content-reveal-fragment", handleRevealFragment);
  });

  $effect(() => {
    const pageId = page.id;
    const handleFindLinksRequest = (event: Event) => {
      const detail = (event as CustomEvent<{ pageId: string }>).detail;
      if (!detail || detail.pageId !== pageId || compact) return;
      void handleSuggestLinks();
    };

    window.addEventListener("page-content-find-links", handleFindLinksRequest);
    return () => window.removeEventListener("page-content-find-links", handleFindLinksRequest);
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
        refreshCollectionAfterMutation();
        refreshPageTrees();
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

  function expandAncestors(blockId: string): boolean {
    let parentId = blockRenderState.parentById.get(blockId) ?? null;
    let nextCollapsed: Set<string> | null = null;
    const seen = new Set<string>();

    while (parentId && !seen.has(parentId)) {
      seen.add(parentId);
      if (collapsedIds.has(parentId)) {
        nextCollapsed ??= new Set(collapsedIds);
        nextCollapsed.delete(parentId);
      }
      parentId = blockRenderState.parentById.get(parentId) ?? null;
    }

    if (!nextCollapsed) return false;
    collapsedIds = nextCollapsed;
    return true;
  }

  function selectRevealedBlock(blockId: string) {
    if (!blockRenderState.blockById.has(blockId)) return;

    selectedBlockIds = new Set([blockId]);
    claimBlockSelectionOwner();
    focusedBlockId = null;
    threadBlockId = blockId;
    emitFocusChanged(blockId);
    promotedSelectionCopyText = null;
    selectionMenu = null;

    const active = document.activeElement as HTMLElement | null;
    if (active && (active.isContentEditable || active.closest(".cm-editor"))) {
      active.blur();
    }

    if (revealedBlockTimer !== undefined) {
      window.clearTimeout(revealedBlockTimer);
    }
    revealedBlockId = null;
    requestAnimationFrame(() => {
      revealedBlockId = blockId;
      revealedBlockTimer = window.setTimeout(() => {
        if (revealedBlockId === blockId) {
          revealedBlockId = null;
        }
      }, 2200);
    });
  }

  // Shared empty array for the disabled and depth-0 cases, so rendering a
  // page does not allocate one per block per frame.
  const NO_GUIDES: boolean[] = [];

  function getBlockGuides(blockId: string): boolean[] {
    if (!showBlockGuides) return NO_GUIDES;
    return getAncestorGuides(
      blockId,
      blockRenderState.parentById,
      blockRenderState.depthById,
      blockRenderState.isLastChildById,
    );
  }

  const threadById = $derived.by(() => {
    if (!showBlockGuides || !threadBlockId) return new Map();
    return buildBulletThreadRoles(
      threadBlockId,
      blockRenderState.parentById,
      blockRenderState.childrenByParent,
      collapsedIds,
      visibleBlocks.map((block) => block.id),
    );
  });

  function getBlockDepth(blockId: string): number {
    return blockRenderState.depthById.get(blockId) ?? 0;
  }

  function trackBlockHeight(node: HTMLElement, options: { blockId: string; enabled: boolean }) {
    let currentBlockId = options.blockId;
    let resizeObserver: ResizeObserver | undefined;

    const update = () => {
      const nextHeight = Math.max(1, Math.ceil(node.getBoundingClientRect().height)) + BLOCK_SHELL_GAP;
      if (blockHeights.get(currentBlockId) === nextHeight) return;
      blockHeights.set(currentBlockId, nextHeight);
    };

    const configure = (next: typeof options) => {
      currentBlockId = next.blockId;
      if (!next.enabled) {
        resizeObserver?.disconnect();
        resizeObserver = undefined;
        return;
      }
      if (!resizeObserver) {
        resizeObserver = new ResizeObserver(update);
        resizeObserver.observe(node);
      }
      update();
    };
    configure(options);

    return {
      update: configure,
      destroy() {
        resizeObserver?.disconnect();
      },
    };
  }

  function getRenderedBlockEl(blockId: string): HTMLElement | null {
    return document.querySelector(`[data-block-id="${blockId}"]`) as HTMLElement | null;
  }

  function growProgressiveBookRenderWindow(forceIndex?: number | null): boolean {
    if (!shouldProgressivelyRenderBook || renderedAllVisibleBlocks) return false;
    const next = nextProgressiveRenderLimit(
      visibleBlocks.length,
      progressiveBookRenderLimit,
      BOOK_RENDER_BATCH_COUNT,
      forceIndex,
    );
    if (next === progressiveBookRenderLimit) return false;
    progressiveBookRenderLimit = next;
    return true;
  }

  function maybeGrowProgressiveBookRenderWindow(scroller: HTMLElement | null) {
    if (!scroller || !shouldProgressivelyRenderBook || renderedAllVisibleBlocks) return;
    const distanceToRenderedEnd = scroller.scrollHeight - (scroller.scrollTop + scroller.clientHeight);
    if (distanceToRenderedEnd > BOOK_LOAD_AHEAD_PX) return;
    growProgressiveBookRenderWindow();
  }

  async function ensureBlockRendered(blockId: string): Promise<boolean> {
    if (!isBlockVisible(blockId)) return false;
    const visibleIndex = blockRenderState.visibleIndexById.get(blockId);
    if (
      shouldProgressivelyRenderBook &&
      visibleIndex !== undefined &&
      visibleIndex >= visibleRenderedBlockCount
    ) {
      growProgressiveBookRenderWindow(visibleIndex);
      await tick();
    }
    if (getRenderedBlockEl(blockId)) return true;

    windowAnchorBlockId = blockId;
    await tick();
    return getRenderedBlockEl(blockId) !== null;
  }

  async function revealBlock(
    blockId: string,
    align: ScrollLogicalPosition = "nearest"
  ): Promise<boolean> {
    if (!isBlockVisible(blockId) && expandAncestors(blockId)) {
      await tick();
    }
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

  function headingFragmentForBlock(block: Block): string | null {
    const match = block.content.trim().match(/^#{1,6}\s+(.+)$/);
    return match ? markdownHeadingSlug(match[1]) : null;
  }

  function blockStartsWithFragment(block: Block, fragment: string): boolean {
    const text = block.content
      .trim()
      .replace(/^#{1,6}\s+/, "")
      .replace(/^\|.*$/ms, "")
      .trim();
    if (!text) return false;
    const blockFragment = markdownHeadingSlug(text.slice(0, 160));
    return blockFragment === fragment || blockFragment.startsWith(`${fragment}-`);
  }

  async function revealHeadingFragments(fragments: string[]) {
    const seen = new Set<string>();
    for (const fragment of fragments) {
      const normalized = fragment.trim().replace(/^#/, "");
      if (!normalized || seen.has(normalized)) continue;
      seen.add(normalized);
      if (await revealHeadingFragment(normalized)) return;
    }
  }

  async function revealHeadingFragment(fragment: string): Promise<boolean> {
    const targetFragment = fragment.trim().replace(/^#/, "");
    if (!targetFragment) return false;

    const targetBlock = blocks.find(
      (block) =>
        headingFragmentForBlock(block) === targetFragment ||
        blockStartsWithFragment(block, targetFragment)
    );
    if (targetBlock) {
      await revealBlock(targetBlock.id, "start");
      await tick();
    }

    const target =
      blocksViewportEl?.querySelector(`[id="${CSS.escape(targetFragment)}"]`) ??
      document.getElementById(targetFragment);
    if (target instanceof HTMLElement) {
      target.scrollIntoView({ block: "start" });
      return true;
    }
    return targetBlock !== undefined;
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

  function setUnifiedEditorPrototype(enabled: boolean) {
    useUnifiedEditorPrototype = enabled;
    try {
      localStorage.setItem(unifiedEditorPrototypeKey(page.id), enabled ? "1" : "0");
    } catch {
      // Keep the in-memory toggle working if localStorage is unavailable.
    }
  }

  async function reloadCurrentPageContent(options: { loadCandidates?: boolean } = {}) {
    const request = currentPageLoad();
    const shouldLoadCandidates = options.loadCandidates ?? linkCandidatesRevealed;
    await loadBlocks(request);
    await loadBacklinks(request);
    if (shouldLoadCandidates) {
      await loadLinkCandidates(request);
    }
    await loadHierarchy(request);
  }

  // Register undo callback for THIS page (supports multiple instances in journal view)
  $effect(() => {
    if (page?.id) {
      setUndoCallback(page.id, (_action: UndoAction) => {
        if (_action.type === "accept_link_candidates") {
          lastLinkCandidateAction = null;
        }
        void loadBlocks(currentPageLoad());
        refreshCollectionAfterMutation();
        refreshPageTrees();
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
      linkCandidates = [];
      linkCandidatesError = "";
      linkCandidatesLoading = false;
      linkCandidatesRevealed = false;
      lastLinkCandidateAction = null;
      clearBlockSelection();
      focusedBlockId = null;
      threadBlockId = null;
      emitFocusChanged(null);
      void loadBlocks(request);
      void loadBacklinks(request);
      void loadHierarchy(request);
    }
  });

  $effect(() => {
    const pageId = page?.id;
    if (!pageId || compact) {
      collectionKind = null;
      collectionMemberCount = undefined;
      collectionStatus = "page";
      return;
    }
    collectionKind = getCollectionKind(page.properties);
    collectionMemberCount = undefined;
    void loadCollection(pageId);
  });

  $effect(() => {
    const pageId = page.id;
    if (compact) return;
    const refreshCollection = (event: Event) => {
      const detail = (event as CustomEvent<{ pageId?: string }>).detail;
      if (!detail?.pageId || detail.pageId === pageId) void loadCollection(pageId);
    };
    window.addEventListener("page-collection-refresh", refreshCollection);
    return () => window.removeEventListener("page-collection-refresh", refreshCollection);
  });

  async function loadCollection(pageId: string) {
    const request = ++collectionRequest;
    collectionStatus = "loading";
    try {
      const result = await withMissingCommandFallback(
        () => pagesListCollections(),
        [],
      );
      if (request !== collectionRequest || page.id !== pageId) return;
      if (!result.available) {
        collectionStatus = "unavailable";
        return;
      }
      const summary = result.value.find((collection) => collection.id === pageId);
      collectionKind = summary?.kind ?? null;
      collectionMemberCount = summary?.member_count ?? 0;
      collectionStatus = summary ? "collection" : "page";
    } catch (error) {
      if (request !== collectionRequest || page.id !== pageId) return;
      collectionStatus = "error";
      console.warn("[collection] Failed to load page collection:", error);
    }
  }

  function refreshCollectionAfterMutation() {
    if (!compact && collectionKind !== null && collectionStatus !== "unavailable") {
      void loadCollection(page.id);
    }
  }

  function refreshPageTrees() {
    window.dispatchEvent(new CustomEvent("page-tree-refresh"));
  }

  async function updateCollection(kind: string | null) {
    if (collectionBusy || collectionStatus === "loading" || collectionStatus === "unavailable") {
      return;
    }
    collectionBusy = true;
    try {
      await pageSetCollection(page.id, kind);
      await loadCollection(page.id);
      window.dispatchEvent(new CustomEvent("page-tree-refresh"));
    } catch (error) {
      collectionStatus = "error";
      console.warn("[collection] Failed to update page kind:", error);
    } finally {
      collectionBusy = false;
    }
  }

  function navigateToCollectionMember(title: string) {
    window.dispatchEvent(new CustomEvent("navigate-page", { detail: title }));
  }

  async function loadBlocks(request: PageLoadRequest = currentPageLoad()) {
    try {
      if (isCurrentPageLoad(pageLoadState, request)) {
        loadError = null;
        windowAnchorBlockId = null;
        blockHeights.clear();
        materializedFilePath = null;
        progressiveBookRenderLimit = BOOK_INITIAL_RENDER_COUNT;
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

      // Creating that first block is what writes the page's markdown file, so
      // the page object we were handed can be out of date. Asked on every
      // reload rather than only when the page started empty: reloading after
      // an undo finds blocks already present, and only refreshing in the empty
      // case left the path null for good — media on the page then resolved
      // against the graph root and broke.
      if (!page.file_path) {
        const refreshed = await getPage({ id: request.pageId }).catch(() => null);
        if (!isCurrentPageLoad(pageLoadState, request)) return;
        materializedFilePath = refreshed?.file_path ?? null;
      }
    } catch (e: any) {
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      loadError = e?.toString() || "Unknown error loading blocks";
      console.error("loadBlocks failed:", e);
    } finally {
      if (onLoadSettled) {
        await tick();
        if (!destroyed && isCurrentPageLoad(pageLoadState, request)) onLoadSettled();
      }
    }
  }

  async function loadLinkCandidates(
    request: PageLoadRequest = currentPageLoad(),
    scan = false
  ) {
    if (compact) {
      linkCandidates = [];
      linkCandidatesError = "";
      linkCandidatesLoading = false;
      return;
    }

    linkCandidatesLoading = true;
    linkCandidatesError = "";
    try {
      const candidates = scan
        ? await discoverLinkCandidates(request.pageId, LINK_CANDIDATE_REVIEW_LIMIT)
        : await listLinkCandidates(request.pageId, "pending", LINK_CANDIDATE_REVIEW_LIMIT);
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      linkCandidates = candidates;
    } catch (e: any) {
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      linkCandidatesError = e?.toString() || "Failed to load link suggestions";
    } finally {
      if (isCurrentPageLoad(pageLoadState, request)) {
        linkCandidatesLoading = false;
      }
    }
  }

  async function handleFindLinks() {
    if (linkCandidatesLoading || conceptEdgeBusy) return;
    linkCandidatesRevealed = true;
    await loadLinkCandidates(currentPageLoad(), true);
  }

  async function handleSuggestLinks() {
    if (linkCandidatesLoading || conceptEdgeBusy) return;
    const request = currentPageLoad();
    linkCandidatesRevealed = true;
    if (blocks.length === 0) {
      await loadBlocks(request);
      if (!isCurrentPageLoad(pageLoadState, request) || blocks.length === 0) return;
    }
    if (!isImportedBookPage) {
      await loadLinkCandidates(request, true);
      if (!isCurrentPageLoad(pageLoadState, request)) return;
    }
    await handleCreateConceptEdges();
  }

  const LINK_CANDIDATE_CONTEXT_CHARS = 180;
  const utf8TextEncoder = new TextEncoder();

  function normalizedLinkCandidateText(text: string): string {
    return text.trim().replace(/\s+/g, " ").toLowerCase();
  }

  function normalizedSpellingText(text: string): string {
    return text.toLowerCase().replace(/[^a-z0-9]+/g, "");
  }

  function editDistance(a: string, b: string): number {
    if (a === b) return 0;
    if (a.length === 0) return b.length;
    if (b.length === 0) return a.length;

    const previous = Array.from({ length: b.length + 1 }, (_, index) => index);
    const current = Array.from({ length: b.length + 1 }, () => 0);

    for (let i = 1; i <= a.length; i += 1) {
      current[0] = i;
      for (let j = 1; j <= b.length; j += 1) {
        const substitutionCost = a[i - 1] === b[j - 1] ? 0 : 1;
        current[j] = Math.min(
          previous[j] + 1,
          current[j - 1] + 1,
          previous[j - 1] + substitutionCost
        );
      }
      for (let j = 0; j <= b.length; j += 1) {
        previous[j] = current[j];
      }
    }

    return previous[b.length];
  }

  function looksLikeSpellingCorrection(candidate: LinkCandidate): boolean {
    if (candidate.source !== "semantic_concept") return false;
    const anchor = normalizedSpellingText(candidate.anchor_text);
    const target = normalizedSpellingText(candidate.to_page_title);
    if (!anchor || !target || anchor === target) return false;
    if (anchor[0] !== target[0]) return false;

    const distance = editDistance(anchor, target);
    const maxDistance = Math.max(1, Math.floor(Math.max(anchor.length, target.length) * 0.18));
    return distance <= Math.min(maxDistance, 3);
  }

  function usesCanonicalTarget(candidate: LinkCandidate): boolean {
    return candidate.source === "semantic_concept"
      && normalizedLinkCandidateText(candidate.anchor_text) !== normalizedLinkCandidateText(candidate.to_page_title);
  }

  function utf8ByteOffsetToStringIndex(text: string, byteOffset: number): number {
    const target = Math.max(0, byteOffset);
    let bytes = 0;
    for (let index = 0; index < text.length;) {
      if (bytes >= target) return index;
      const codePoint = text.codePointAt(index);
      if (codePoint === undefined) return text.length;
      const char = String.fromCodePoint(codePoint);
      const nextBytes = bytes + utf8TextEncoder.encode(char).length;
      if (nextBytes > target) return index;
      bytes = nextBytes;
      index += char.length;
    }
    return text.length;
  }

  function candidateRangeInContent(candidate: LinkCandidate, content: string): { start: number; end: number } | null {
    const start = utf8ByteOffsetToStringIndex(content, candidate.anchor_start);
    const end = utf8ByteOffsetToStringIndex(content, candidate.anchor_end);
    if (start <= end && content.slice(start, end) === candidate.anchor_text) {
      return { start, end };
    }

    const fallbackStart = content.toLowerCase().indexOf(candidate.anchor_text.toLowerCase());
    if (fallbackStart >= 0) {
      return { start: fallbackStart, end: fallbackStart + candidate.anchor_text.length };
    }
    return null;
  }

  function normalizeContextText(text: string): string {
    return text.replace(/\s+/g, " ").trim();
  }

  function candidateDocumentPosition(candidate: LinkCandidate): { blockIndex: number; anchorStart: number } {
    const visibleIndex = blockRenderState.visibleIndexById.get(candidate.from_block_id);
    const blockIndex = visibleIndex ?? blocks.findIndex((block) => block.id === candidate.from_block_id);
    return {
      blockIndex: blockIndex < 0 ? Number.MAX_SAFE_INTEGER : blockIndex,
      anchorStart: candidate.anchor_start,
    };
  }

  function firstCandidateForReview(group: LinkCandidateGroup): LinkCandidate {
    return uniqueOccurrenceCandidates(group.candidates)
      .sort((a, b) => {
        const aPos = candidateDocumentPosition(a);
        const bPos = candidateDocumentPosition(b);
        if (aPos.blockIndex !== bPos.blockIndex) return aPos.blockIndex - bPos.blockIndex;
        return aPos.anchorStart - bPos.anchorStart;
      })[0] ?? group.primary;
  }

  function linkCandidateContextPreview(group: LinkCandidateGroup): LinkCandidateContextPreview | null {
    const candidate = firstCandidateForReview(group);
    const block = blockRenderState.blockById.get(candidate.from_block_id)
      ?? blocks.find((item) => item.id === candidate.from_block_id);
    if (!block) return null;

    const range = candidateRangeInContent(candidate, block.content);
    if (!range) {
      const blockIndex = blocks.findIndex((item) => item.id === candidate.from_block_id);
      return {
        candidate,
        before: normalizeContextText(block.content.slice(0, LINK_CANDIDATE_CONTEXT_CHARS * 2)),
        anchor: candidate.anchor_text,
        after: "",
        leadingEllipsis: false,
        trailingEllipsis: block.content.length > LINK_CANDIDATE_CONTEXT_CHARS * 2,
        blockLabel: blockIndex >= 0 ? `Block ${blockIndex + 1}` : "Matching block",
      };
    }

    const previewStart = Math.max(0, range.start - LINK_CANDIDATE_CONTEXT_CHARS);
    const previewEnd = Math.min(block.content.length, range.end + LINK_CANDIDATE_CONTEXT_CHARS);
    const blockIndex = blocks.findIndex((item) => item.id === candidate.from_block_id);
    return {
      candidate,
      before: normalizeContextText(block.content.slice(previewStart, range.start)),
      anchor: normalizeContextText(block.content.slice(range.start, range.end)) || candidate.anchor_text,
      after: normalizeContextText(block.content.slice(range.end, previewEnd)),
      leadingEllipsis: previewStart > 0,
      trailingEllipsis: previewEnd < block.content.length,
      blockLabel: blockIndex >= 0 ? `Block ${blockIndex + 1}` : "Matching block",
    };
  }

  async function handleRevealLinkCandidateGroup(group: LinkCandidateGroup) {
    await revealLinkCandidateOccurrence(firstCandidateForReview(group));
  }

  async function revealLinkCandidateOccurrence(candidate: LinkCandidate) {
    linkCandidatesError = "";
    if (linkCandidateRevealTimer !== undefined) {
      window.clearTimeout(linkCandidateRevealTimer);
      linkCandidateRevealTimer = undefined;
    }

    const revealed = await revealBlock(candidate.from_block_id, "center");
    if (!revealed) {
      linkCandidatesError = "Could not reveal this occurrence; refresh suggestions and try again.";
      linkCandidatesRevealed = true;
      return;
    }

    selectRevealedBlock(candidate.from_block_id);
    await tick();

    const blockEl = getRenderedBlockEl(candidate.from_block_id);
    if (!blockEl) return;
    const firstMatch = highlightTerm(blockEl, candidate.anchor_text);
    firstMatch?.scrollIntoView({ block: "center", behavior: "smooth" });
    linkCandidateRevealTimer = window.setTimeout(() => {
      const currentBlockEl = getRenderedBlockEl(candidate.from_block_id);
      if (currentBlockEl) clearHighlights(currentBlockEl);
      linkCandidateRevealTimer = undefined;
    }, 4500);
  }

  function linkCandidateOccurrenceKey(candidate: LinkCandidate): string {
    return `${candidate.from_block_id}:${candidate.to_page_id}:${candidate.anchor_start}:${candidate.anchor_end}`;
  }

  function uniqueOccurrenceCandidates(candidates: LinkCandidate[]): LinkCandidate[] {
    const seen = new Set<string>();
    const unique: LinkCandidate[] = [];
    for (const candidate of candidates) {
      const key = linkCandidateOccurrenceKey(candidate);
      if (seen.has(key)) continue;
      seen.add(key);
      unique.push(candidate);
    }
    return unique;
  }

  function sortedCandidatesForAccept(candidates: LinkCandidate[]): LinkCandidate[] {
    return uniqueOccurrenceCandidates(candidates).sort((a, b) => {
      if (a.from_block_id !== b.from_block_id) {
        return a.from_block_id.localeCompare(b.from_block_id);
      }
      return b.anchor_start - a.anchor_start;
    });
  }

  function groupLinkCandidates(candidates: LinkCandidate[]): LinkCandidateGroup[] {
    const groups = new Map<string, LinkCandidateGroup>();

    for (const candidate of candidates) {
      const key = `${candidate.from_page_id}:${candidate.to_page_id}`;
      const group = groups.get(key);
      if (group) {
        group.candidates.push(candidate);
        if (!group.sources.includes(candidate.source)) group.sources.push(candidate.source);
        if (!group.anchorTexts.some((text) => normalizedLinkCandidateText(text) === normalizedLinkCandidateText(candidate.anchor_text))) {
          group.anchorTexts.push(candidate.anchor_text);
        }
        group.canFixSpelling = group.canFixSpelling || looksLikeSpellingCorrection(candidate);
        group.hasCanonicalTarget = group.hasCanonicalTarget || usesCanonicalTarget(candidate);
      } else {
        groups.set(key, {
          key,
          candidates: [candidate],
          primary: candidate,
          occurrenceCount: 1,
          anchorTexts: [candidate.anchor_text],
          sources: [candidate.source],
          canFixSpelling: looksLikeSpellingCorrection(candidate),
          hasCanonicalTarget: usesCanonicalTarget(candidate),
        });
      }
    }

    for (const group of groups.values()) {
      group.occurrenceCount = uniqueOccurrenceCandidates(group.candidates).length;
      group.candidates.sort((a, b) => b.updated_at - a.updated_at);
      group.primary = group.candidates[0] ?? group.primary;
    }

    return Array.from(groups.values()).sort((a, b) => b.primary.updated_at - a.primary.updated_at);
  }

  async function handleAcceptLinkCandidateGroup(group: LinkCandidateGroup) {
    linkCandidatesError = "";
    const accepted: LinkCandidate[] = [];
    try {
      const candidatesToAccept = sortedCandidatesForAccept(group.candidates);
      for (const candidate of candidatesToAccept) {
        accepted.push(await acceptLinkCandidate(candidate.id));
      }
      const acceptedIds = new Set(accepted.map((candidate) => candidate.id));
      const duplicateCandidates = group.candidates.filter((candidate) => !acceptedIds.has(candidate.id));
      await Promise.all(duplicateCandidates.map((candidate) => dismissLinkCandidate(candidate.id)));
      lastLinkCandidateAction = { kind: "accepted", candidates: accepted };
      if (accepted.length > 0) {
        pushUndo({
          type: "accept_link_candidates",
          pageId: page.id,
          candidateIds: accepted.map((candidate) => candidate.id),
        });
      }
      linkCandidatesRevealed = true;
      await reloadCurrentPageContent({ loadCandidates: true });
      refreshPageTrees();
    } catch (e: any) {
      if (accepted.length > 0) {
        lastLinkCandidateAction = { kind: "accepted", candidates: accepted };
      }
      linkCandidatesError = e?.toString() || "Failed to link suggestion";
      linkCandidatesRevealed = true;
      await loadLinkCandidates(currentPageLoad(), true);
    }
  }

  async function handleDismissLinkCandidateGroup(group: LinkCandidateGroup) {
    linkCandidatesError = "";
    try {
      const dismissed = await Promise.all(group.candidates.map((candidate) => dismissLinkCandidate(candidate.id)));
      lastLinkCandidateAction = { kind: "dismissed", candidates: dismissed };
      const dismissedIds = new Set(dismissed.map((candidate) => candidate.id));
      linkCandidates = linkCandidates.filter((item) => !dismissedIds.has(item.id));
    } catch (e: any) {
      linkCandidatesError = e?.toString() || "Failed to dismiss suggestion";
      linkCandidatesRevealed = true;
      await loadLinkCandidates(currentPageLoad(), true);
    }
  }

  async function undoLastLinkCandidateAction() {
    if (!lastLinkCandidateAction) return;
    const action = lastLinkCandidateAction;
    linkCandidatesError = "";
    try {
      if (action.kind === "accepted") {
        for (const candidate of [...action.candidates].reverse()) {
          await undoLinkCandidateAccept(candidate.id);
        }
        const undoneIds = new Set(action.candidates.map((candidate) => candidate.id));
        removeUndoActions((undo) =>
          undo.type === "accept_link_candidates" &&
          undo.candidateIds.length === undoneIds.size &&
          undo.candidateIds.every((id) => undoneIds.has(id))
        );
        linkCandidatesRevealed = true;
        await reloadCurrentPageContent({ loadCandidates: true });
      } else {
        await Promise.all(action.candidates.map((candidate) => restoreLinkCandidate(candidate.id)));
        linkCandidatesRevealed = true;
        await loadLinkCandidates();
      }
      lastLinkCandidateAction = null;
      refreshPageTrees();
    } catch (e: any) {
      linkCandidatesError = e?.toString() || "Failed to undo link suggestion action";
    }
  }

  function navigateToCandidateTarget(candidate: LinkCandidate) {
    window.dispatchEvent(new CustomEvent("navigate-page", { detail: candidate.to_page_title }));
  }

  function linkCandidateSourceLabel(candidate: LinkCandidate): string {
    return candidate.source === "semantic_concept" ? "AI concept" : "Exact mention";
  }

  function linkCandidateGroupSourceLabel(group: LinkCandidateGroup): string {
    if (group.hasCanonicalTarget) return "Alias match";
    if (group.sources.length > 1) return "Mixed";
    return linkCandidateSourceLabel(group.primary);
  }

  function linkCandidateAnchorSummary(group: LinkCandidateGroup): string {
    const quoted = group.anchorTexts.slice(0, 3).map((text) => `"${text}"`);
    const extra = group.anchorTexts.length > 3 ? ` +${group.anchorTexts.length - 3} more` : "";
    return `${quoted.join(", ")}${extra}`;
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
      const seenBlockIds = new Set<string>();
      const uniqueBacklinkResults = backlinkResults.filter((result) => {
        if (result.block.page_id === request.pageId) return false;
        if (seenBlockIds.has(result.block.id)) return false;
        seenBlockIds.add(result.block.id);
        return true;
      });

      const uniquePageIds = Array.from(new Set(uniqueBacklinkResults.map((r) => r.block.page_id)));
      const blocksPromiseCache = new Map<string, Promise<Block[]>>();
      const pagePromiseCache = new Map<string, Promise<Page | null>>();
      for (const pageId of uniquePageIds) {
        blocksPromiseCache.set(pageId, listBlocks(pageId));
        pagePromiseCache.set(pageId, getPage({ id: pageId }).catch(() => null));
      }

      const indexCache = new Map<string, Promise<BacklinkSourceIndex>>();
      for (const pageId of uniquePageIds) {
        indexCache.set(
          pageId,
          blocksPromiseCache.get(pageId)!.then((sourceBlocks) => buildBacklinkSourceIndex(sourceBlocks))
        );
      }

      const renderedBacklinks = await Promise.all(uniqueBacklinkResults.map(async (result) => {
        const [sourcePage, index] = await Promise.all([
          pagePromiseCache.get(result.block.page_id)!,
          indexCache.get(result.block.page_id)!,
        ]);

        return {
          ...result,
          sourcePageTitle: sourcePage?.title ?? "",
          sourceAssetBaseDir: assetBaseDirFor(sourcePage?.file_path),
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

  // Track block content before editing starts, so undo can restore it
  let preEditSnapshots: Map<string, Block> = new Map();

  function handleFocus(blockId: string) {
    focusedBlockId = blockId;
    threadBlockId = blockId;
    emitFocusChanged(blockId);
    clearBlockSelection();
    // Snapshot the block content before the user edits it
    const block = blockRenderState.blockById.get(blockId);
    if (block) {
      preEditSnapshots.set(blockId, { ...block });
    }
  }

  function handleBlur(blockId: string) {
    focusedBlockId = null;
    const before = preEditSnapshots.get(blockId);
    const after = blockRenderState.blockById.get(blockId);
    if (before && after && pageTreeReferencesChanged(before.content, after.content)) {
      refreshPageTrees();
    }
    if (collectionStatus === "collection") {
      void loadCollection(page.id);
    }
    // Don't auto-delete if we're navigating to another block
    if (navigatingBlock) {
      navigatingBlock = false;
      preEditSnapshots.delete(blockId);
      return;
    }
    preEditSnapshots.delete(blockId);
  }

  function snapshotBlock(block: Block): Block {
    return {
      ...block,
      properties: JSON.parse(JSON.stringify(block.properties ?? {})),
    };
  }

  function blocksInTreeOrder(input: readonly Block[]): Block[] {
    const arrayIndex = new Map(input.map((block, index) => [block.id, index]));
    const childrenByParent = new Map<string | null, Block[]>();
    for (const block of input) {
      const siblings = childrenByParent.get(block.parent_id) ?? [];
      siblings.push(block);
      childrenByParent.set(block.parent_id, siblings);
    }
    for (const siblings of childrenByParent.values()) {
      siblings.sort((a, b) => {
        const orderDelta = a.order_index - b.order_index;
        return orderDelta || (arrayIndex.get(a.id) ?? 0) - (arrayIndex.get(b.id) ?? 0);
      });
    }

    const ordered: Block[] = [];
    const seen = new Set<string>();
    const visit = (parentId: string | null) => {
      for (const child of childrenByParent.get(parentId) ?? []) {
        if (seen.has(child.id)) continue;
        seen.add(child.id);
        ordered.push(child);
        visit(child.id);
      }
    };
    visit(null);
    for (const block of input) {
      if (!seen.has(block.id)) ordered.push(block);
    }
    return ordered;
  }

  function blocksWithDescendantsInDocumentOrder(rootIds: ReadonlySet<string>): Block[] {
    const selectedOrDescendant = new Set<string>();
    const orderedBlocks = blocksInTreeOrder(blocks);
    const subtreeBlocks: Block[] = [];

    for (const block of orderedBlocks) {
      if (rootIds.has(block.id) || (block.parent_id && selectedOrDescendant.has(block.parent_id))) {
        selectedOrDescendant.add(block.id);
        subtreeBlocks.push(block);
      }
    }

    return subtreeBlocks;
  }

  function undoSnapshotsForBlocks(sourceBlocks: readonly Block[]): Block[] {
    return sourceBlocks.map((block) => snapshotBlock(preEditSnapshots.get(block.id) ?? block));
  }

  function pushBlockContentUndo(pageId: string, changes: BlockContentChange[]) {
    const effective = changes.filter((change) => change.beforeContent !== change.afterContent);
    if (effective.length === 0) return;
    if (effective.length === 1) {
      const change = effective[0];
      pushUndo({
        type: "update_block",
        pageId,
        blockId: change.blockId,
        beforeContent: change.beforeContent,
        afterContent: change.afterContent,
      });
      return;
    }

    pushUndo({
      type: "update_blocks",
      pageId,
      changes: effective,
    });
  }

  function reflectCurrentPageContentChanges(changes: BlockContentChange[]) {
    const contentById = new Map(changes.map((change) => [change.blockId, change.afterContent]));
    blocks = blocks.map((block) => {
      const content = contentById.get(block.id);
      return content === undefined ? block : { ...block, content };
    });
    refreshCollectionAfterMutation();
    if (changes.some((change) => pageTreeReferencesChanged(change.beforeContent, change.afterContent))) {
      refreshPageTrees();
    }
  }

  async function applyCurrentPageContentChanges(changes: BlockContentChange[]): Promise<boolean> {
    const effective = changes.filter((change) => change.beforeContent !== change.afterContent);
    if (effective.length === 0) return false;

    for (const change of effective) {
      await updateBlock(change.blockId, change.afterContent);
    }

    pushBlockContentUndo(page.id, effective);
    reflectCurrentPageContentChanges(effective);
    return true;
  }

  function handleBlockContentChange(changedPageId: string, change: BlockContentChange) {
    if (change.beforeContent === change.afterContent) return;
    pushBlockContentUndo(changedPageId, [change]);
    if (changedPageId === page.id) {
      reflectCurrentPageContentChanges([change]);
    }
  }

  async function handleEnter(blockId: string, content: string, _orderIndex: number, atStart: boolean, remainder = "") {
    try {
      const block = blocks.find((b) => b.id === blockId);
      if (!block) return;

      // Persist the current block content before any structural operation
      // (create/move), otherwise write-page operations can serialize stale empty text.
      if (block.content !== content) {
        await applyCurrentPageContentChanges([{
          blockId,
          beforeContent: block.content,
          afterContent: content,
        }]);
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
        refreshCollectionAfterMutation();

        requestAnimationFrame(() => {
          focusedBlockId = newBlock.id;
          threadBlockId = newBlock.id;
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

      const newBlock = await createBlock(page.id, parentId, newOrder, remainder);
      // Insert after current block in the array
      const idx = blocks.findIndex((b) => b.id === blockId);
      blocks = [...blocks.slice(0, idx + 1), newBlock, ...blocks.slice(idx + 1)];
      refreshCollectionAfterMutation();
      // Focus the new block
      requestAnimationFrame(() => {
        focusedBlockId = newBlock.id;
        threadBlockId = newBlock.id;
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

  async function handlePasteBlocks(
    blockId: string,
    pasteBlocks: import("../lib/htmlToMd").PasteBlock[],
    anchorEdit?: { beforeContent: string; afterContent: string },
    persistAnchor?: () => Promise<void>,
  ) {
    const request = currentPageLoad();
    const originalBlocks = blocks;
    const isCurrent = () => !destroyed && page.id === request.pageId && isCurrentPageLoad(pageLoadState, request);
    try {
      const idx = blocks.findIndex((b) => b.id === blockId);
      const block = blocks[idx];
      if (!block) throw new Error("The paste destination is no longer available.");

      const anchorBeforeContent = anchorEdit?.beforeContent ?? preEditSnapshots.get(blockId)?.content ?? block.content;
      const anchorAfterContent = anchorEdit?.afterContent ?? block.content;
      const baseParentId = block.parent_id;
      const batch: CreateBlockBatchItem[] = [];
      type ParentRef = { kind: "existing"; id: string | null } | { kind: "new"; index: number };
      // Track parent at each depth level. depth 0 siblings share baseParentId,
      // depth 1 items are children of the current block because the first pasted
      // chunk is inserted into it, and deeper items attach to newly-created
      // parents from the batch.
      const parentAtDepth: ParentRef[] = [{ kind: "existing", id: baseParentId }, { kind: "existing", id: blockId }];
      const orderAtDepth: number[] = [
        block.order_index + 1,
        blocks.filter((candidate) => candidate.parent_id === blockId).length,
      ];

      for (const pb of pasteBlocks) {
        const depth = pb.depth;
        const fallbackParentRef: ParentRef = { kind: "existing", id: baseParentId };
        // Determine parent: if depth > 0, parent is the last block at depth-1
        const parentRef: ParentRef =
          depth > 0
            ? (parentAtDepth[depth] ?? parentAtDepth[parentAtDepth.length - 1] ?? fallbackParentRef)
            : fallbackParentRef;

        // Get order index for this depth
        if (!orderAtDepth[depth]) orderAtDepth[depth] = 0;
        const order = orderAtDepth[depth]!;
        orderAtDepth[depth] = order + 1;

        const batchIndex = batch.length;
        batch.push({
          parentId: parentRef.kind === "existing" ? parentRef.id : undefined,
          parentIndex: parentRef.kind === "new" ? parentRef.index : undefined,
          orderIndex: order,
          content: pb.content,
        });

        // This block can be a parent for deeper items
        parentAtDepth[depth + 1] = { kind: "new", index: batchIndex };
        // Reset child order counters for deeper levels
        for (let d = depth + 1; d < orderAtDepth.length; d++) {
          orderAtDepth[d] = 0;
        }
      }
      // Capture the destination before saving the anchor can yield to navigation.
      await persistAnchor?.();
      let newBlocks = await createBlocks(request.pageId, batch);
      let updatedBlocks = isCurrent() ? blocks : originalBlocks;
      let finalSiblingIds: string[] = [];
      const siblingInsertCount = pasteBlocks.filter((pb) => pb.depth === 0).length;
      if (siblingInsertCount > 0) {
        const pastedSiblingIds = newBlocks
          .filter((newBlock) => newBlock.parent_id === baseParentId)
          .sort((a, b) => a.order_index - b.order_index)
          .map((newBlock) => newBlock.id);
        finalSiblingIds = updatedBlocks
          .filter((candidate) => candidate.parent_id === baseParentId)
          .sort((a, b) => a.order_index - b.order_index)
          .flatMap((candidate) => candidate.id === blockId ? [candidate.id, ...pastedSiblingIds] : [candidate.id]);
        const orderById = new Map(finalSiblingIds.map((id, order) => [id, order]));
        updatedBlocks = updatedBlocks.map((existing) => {
          const order = orderById.get(existing.id);
          return order === undefined ? existing : { ...existing, order_index: order };
        });
        newBlocks = newBlocks.map((created) => {
          const order = orderById.get(created.id);
          return order === undefined ? created : { ...created, order_index: order };
        });
      }
      pushUndo({
        type: "insert_blocks",
        pageId: request.pageId,
        anchorBlockId: blockId,
        beforeContent: anchorBeforeContent,
        afterContent: anchorAfterContent,
        insertedBlocks: newBlocks.map(snapshotBlock),
      });
      // The batch is already saved. Publish it without waiting for the separate
      // ordering write, whose failure must never hide successfully created text.
      if (isCurrent()) {
        blocks = blocksInTreeOrder([...updatedBlocks, ...newBlocks]);
        refreshCollectionAfterMutation();
      }
      if (finalSiblingIds.length > 0) {
        void reorderBlocks(request.pageId, finalSiblingIds).catch((error) => {
          console.error("Failed to save pasted block order:", error);
          showToast(`Pasted blocks were saved, but their order could not be confirmed: ${error instanceof Error ? error.message : String(error)}`, "error");
        });
      }
      if (pasteBlocks.some((block) => pageTreeReferencesChanged("", block.content))) {
        refreshPageTrees();
      }
      // Focus the last new block
      const lastNew = newBlocks[newBlocks.length - 1];
      if (lastNew && isCurrent() && focusedBlockId === blockId) {
        requestAnimationFrame(() => {
          if (!isCurrent() || focusedBlockId !== blockId) return;
          void revealBlock(lastNew.id).then((rendered) => {
            if (!rendered || !isCurrent() || focusedBlockId !== blockId) return;
            focusedBlockId = lastNew.id;
            blockRefs[lastNew.id]?.focusAtEnd();
          });
        });
      }
    } catch (e) {
      console.error("Failed to paste blocks:", e);
      showToast(`Paste could not be completed: ${e instanceof Error ? e.message : String(e)}`, "error");
      throw e;
    }
  }

  async function handleClickBelow() {
    try {
      const lastOrder = blocks.length > 0 ? blocks[blocks.length - 1].order_index + 1 : 0;
      const newBlock = await createBlock(page.id, null, lastOrder, "");
      blocks = [...blocks, newBlock];
      refreshCollectionAfterMutation();
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
    const subtree = blocksWithDescendantsInDocumentOrder(new Set([blockId]));
    const block = subtree[0];
    if (block) {
      const undoBlocks = undoSnapshotsForBlocks(subtree);
      console.log("[DELETE] pushing to undo stack, content:", undoBlocks[0].content.substring(0, 40));
      await deleteBlocks(page.id, undoBlocks.map((snapshot) => snapshot.id));
      pushUndo({ type: "delete_blocks", blocks: undoBlocks, pageId: page.id });
      for (const snapshot of undoBlocks) {
        preEditSnapshots.delete(snapshot.id);
      }
    } else {
      console.log("[DELETE] block not found!");
      return;
    }
    const idx = blocks.findIndex((b) => b.id === blockId);
    const deletedIds = new Set(subtree.map((item) => item.id));
    const remaining = blocks.filter((b) => !deletedIds.has(b.id));
    if (remaining.length === 0) {
      blocks = [await createBlock(page.id, null, 0, "")];
    } else {
      blocks = remaining;
    }
    refreshCollectionAfterMutation();
    if (subtree.some((item) => pageTreeReferencesChanged(item.content, ""))) refreshPageTrees();
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

  function focusBlockForEditing(blockId: string) {
    requestAnimationFrame(() => {
      focusedBlockId = blockId;
      const el = document.querySelector(`[data-block-id="${blockId}"] .block-content`);
      if (el) {
        el.scrollIntoView({ block: "nearest" });
        (el as HTMLElement).click();
      }
    });
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
      threadBlockId = target.id;
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
      await applyCurrentPageContentChanges([{
        blockId: block.id,
        beforeContent: block.content,
        afterContent: currentContent,
      }]);
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
        refreshCollectionAfterMutation();
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
    promotedSelectionCopyText = null;
    const plan = planIndentSelection(blocks, selectedBlockIds, direction);
    if (plan.moves.length === 0) return; // nothing movable — silent no-op

    const keep = new Set(selectedBlockIds);
    try {
      for (const move of plan.moves) {
        await moveBlock(move.id, move.newParentId, move.newOrderIndex);
      }
      blocks = plan.blocks;
      selectedBlockIds = keep;
      refreshCollectionAfterMutation();
    } catch (e) {
      console.error("Failed to indent/outdent selection:", e);
    }
  }

  function handleBulletClick(blockId: string, event: MouseEvent) {
    promotedSelectionCopyText = null;
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
    if (selectedBlockIds.size > 0) {
      claimBlockSelectionOwner();
    } else {
      releaseBlockSelectionOwner();
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
    const selectedIds = new Set(selectedBlockIds);
    const blocksToDelete = blocksWithDescendantsInDocumentOrder(selectedIds);
    if (blocksToDelete.length === 0) return;
    const deletedIds = new Set(blocksToDelete.map((block) => block.id));
    const firstDeletedIdx = blocks.findIndex((b) => deletedIds.has(b.id));

    // Save deleted blocks for undo
    const deletedBlocks = undoSnapshotsForBlocks(blocksToDelete);
    await deleteBlocks(page.id, deletedBlocks.map((block) => block.id));
    pushUndo({ type: "delete_blocks", blocks: deletedBlocks, pageId: page.id });

    for (const block of deletedBlocks) {
      preEditSnapshots.delete(block.id);
    }

    let focusAfterDelete: Block | null = null;
    const remaining = blocks.filter((b) => !deletedIds.has(b.id));
    if (remaining.length === 0) {
      // All blocks deleted — create a fresh empty block
      const newBlock = await createBlock(page.id, null, 0, "");
      blocks = [newBlock];
      focusAfterDelete = newBlock;
    } else {
      blocks = remaining;
      focusAfterDelete =
        remaining[Math.min(Math.max(firstDeletedIdx, 0), remaining.length - 1)] ??
        remaining[remaining.length - 1] ??
        null;
    }
    clearBlockSelection();
    refreshCollectionAfterMutation();
    if (deletedBlocks.some((block) => pageTreeReferencesChanged(block.content, ""))) {
      refreshPageTrees();
    }
    if (focusAfterDelete) {
      focusBlockForEditing(focusAfterDelete.id);
    }
  }

  let analyzingSelection = $state(false);
  let analyzeSelectionError = $state("");
  let analyzeSelectionProgress = $state("");
  let startingConceptEdges = $state(false);
  let conceptEdgesError = $state("");
  let conceptEdgesMessage = $state("");
  let seenFinishedConceptEdgeJobs = new Set<string>();
  const conceptEdgeJob = $derived.by(() =>
    jobs.find(
      (job) =>
        job.kind === "ai_concept_edges" &&
        job.link?.page_id === page.id &&
        job.status === "running"
    )
  );
  const conceptEdgeBusy = $derived(startingConceptEdges || !!conceptEdgeJob);
  const conceptEdgeProgress = $derived(
    conceptEdgeJob?.message ?? (startingConceptEdges ? "Starting concept edge job..." : "")
  );
  const suggestLinksButtonLabel = $derived(
    linkCandidatesLoading ? "Scanning..." : conceptEdgeBusy ? "Finding..." : "Suggest links"
  );

  $effect(() => {
    const pageId = page.id;
    for (const job of jobs) {
      if (
        job.kind !== "ai_concept_edges" ||
        job.link?.page_id !== pageId ||
        !isTerminal(job.status) ||
        seenFinishedConceptEdgeJobs.has(job.id)
      ) {
        continue;
      }

      seenFinishedConceptEdgeJobs.add(job.id);
      if (job.status === "succeeded") {
        conceptEdgesError = "";
        conceptEdgesMessage = job.message ?? "Concept edge discovery finished.";
        linkCandidatesRevealed = true;
        void loadLinkCandidates(currentPageLoad());
        refreshPageTrees();
      } else if (job.status === "failed") {
        conceptEdgesError = job.error ?? "Concept edge discovery failed.";
      }
    }
  });

  async function handleCreateConceptEdges() {
    if (conceptEdgeBusy || linkCandidatesLoading) return;
    const request = currentPageLoad();
    startingConceptEdges = true;
    linkCandidatesRevealed = true;
    conceptEdgesError = "";
    conceptEdgesMessage = "Starting concept edge job...";
    try {
      await aiCreateConceptEdges(request.pageId);
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      conceptEdgesMessage = "Concept edge job started. Watch Jobs for progress.";
    } catch (e) {
      if (!isCurrentPageLoad(pageLoadState, request)) return;
      conceptEdgesError = e instanceof Error ? e.message : String(e);
    } finally {
      if (isCurrentPageLoad(pageLoadState, request)) {
        startingConceptEdges = false;
      }
    }
  }

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
        const wrapChanges: BlockContentChange[] = [];
        for (const block of selected) {
          const wrapped = await wrapKnownTermsInText(block.content, allTags);
          if (wrapped !== block.content) {
            wrapChanges.push({
              blockId: block.id,
              beforeContent: block.content,
              afterContent: wrapped,
            });
          }
        }
        await applyCurrentPageContentChanges(wrapChanges);
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
      pushUndo({
        type: "insert_blocks",
        pageId: page.id,
        anchorBlockId: null,
        beforeContent: null,
        afterContent: null,
        insertedBlocks: created.map(snapshotBlock),
      });
      clearBlockSelection();
      refreshCollectionAfterMutation();
      refreshPageTrees();
    } catch (e) {
      analyzeSelectionError = e instanceof Error ? e.message : String(e);
    } finally {
      unlisten();
      analyzingSelection = false;
      analyzeSelectionProgress = "";
    }
  }

  function selectedBlocksInDocumentOrder(): Block[] {
    return blocks.filter((block) => selectedBlockIds.has(block.id));
  }

  function blocksForSelectionIds(ids: readonly string[]): Block[] {
    const selected = new Set(ids);
    return blocks.filter((block) => selected.has(block.id));
  }

  function contextMenuBlocks(): Block[] {
    return selectionMenu ? blocksForSelectionIds(selectionMenu.blockIds) : selectedBlocksInDocumentOrder();
  }

  function selectedBlocksWithDescendantsInDocumentOrder(): Block[] {
    return blocksWithDescendantsInDocumentOrder(selectedBlockIds);
  }

  function selectedClipboardBlocks() {
    return selectedBlocksWithDescendantsInDocumentOrder().map((block) => ({
      content: block.content,
      depth: getBlockDepth(block.id),
    }));
  }

  function selectedClipboardPayload(): { markdown: string; plainText: string } {
    const clipboardBlocks = selectedClipboardBlocks();
    const markdown = formatBlocksAsOutlineMarkdown(clipboardBlocks);
    const selectedText = promotedSelectionCopyText?.trim();
    const plainText = selectedText || formatBlocksAsPlainText(clipboardBlocks) || markdown;
    return { markdown, plainText };
  }

  function selectedBlockMarkdown(): string {
    return selectedClipboardPayload().markdown;
  }

  function selectedBlockPlainText(): string {
    return selectedClipboardPayload().plainText;
  }

  function countOccurrences(text: string, needle: string): number {
    if (!needle) return 0;
    let count = 0;
    let index = text.indexOf(needle);
    while (index !== -1) {
      count += 1;
      index = text.indexOf(needle, index + needle.length);
    }
    return count;
  }

  function nthIndexOf(text: string, needle: string, occurrence: number): number {
    let remaining = Math.max(0, occurrence);
    let index = text.indexOf(needle);
    while (index !== -1 && remaining > 0) {
      remaining -= 1;
      index = text.indexOf(needle, index + needle.length);
    }
    return index;
  }

  function renderedSelectionPrefix(range: Range): string {
    const startEl = range.startContainer instanceof Element
      ? range.startContainer
      : range.startContainer.parentElement;
    const renderedRoot = startEl?.closest?.(".rendered-content");
    if (!renderedRoot) return "";

    const prefixRange = document.createRange();
    prefixRange.selectNodeContents(renderedRoot);
    prefixRange.setEnd(range.startContainer, range.startOffset);
    const prefix = prefixRange.toString();
    prefixRange.detach();
    return prefix.replace(/\u00a0/g, " ");
  }

  function nativeSelectionMakeLinkAction(): SelectionMakeLinkAction | null {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;

    const range = selection.getRangeAt(0);
    const startBlockId = blockIdFromNode(range.startContainer);
    const endBlockId = blockIdFromNode(range.endContainer);
    if (!startBlockId || startBlockId !== endBlockId) return null;

    const text = selection.toString().replace(/\u00a0/g, " ").trim();
    if (!text || text.includes("\n")) return null;

    const block = blockRenderState.blockById.get(startBlockId);
    if (!block) return null;

    const occurrence = countOccurrences(renderedSelectionPrefix(range), text);
    const from = nthIndexOf(block.content, text, occurrence);

    return {
      blockId: startBlockId,
      from: from >= 0 ? from : null,
      to: from >= 0 ? from + text.length : null,
      text,
    };
  }

  function captureNativeSelectionMakeLinkAction(): SelectionMakeLinkAction | null {
    const action = nativeSelectionMakeLinkAction();
    if (action) {
      lastSelectionMakeLinkAction = { ...action, capturedAt: Date.now() };
    }
    return action;
  }

  function cachedSelectionMakeLinkAction(blockId: string): SelectionMakeLinkAction | null {
    const action = lastSelectionMakeLinkAction;
    if (!action || action.blockId !== blockId) return null;
    if (Date.now() - action.capturedAt > SELECTION_MAKE_LINK_CACHE_MS) {
      lastSelectionMakeLinkAction = null;
      return null;
    }
    const { capturedAt: _capturedAt, ...makeLink } = action;
    return makeLink;
  }

  function currentMakeLinkRange(action: SelectionMakeLinkAction, content: string): { from: number; to: number } | null {
    if (
      action.from !== null &&
      action.to !== null &&
      content.slice(action.from, action.to) === action.text
    ) {
      return { from: action.from, to: action.to };
    }
    const fallback = content.indexOf(action.text);
    return fallback < 0 ? null : { from: fallback, to: fallback + action.text.length };
  }

  async function makeSelectionLink() {
    const action = selectionMenu?.makeLink;
    if (!action) return;

    const block = blockRenderState.blockById.get(action.blockId);
    if (!block) {
      showSelectionCopyMessage("Block changed; select the text again");
      selectionMenu = null;
      return;
    }

    const range = currentMakeLinkRange(action, block.content);
    if (!range) {
      showSelectionCopyMessage("Text changed; select it again");
      selectionMenu = null;
      return;
    }

    const linked = wrapPageLinkText(block.content, range.from, range.to);
    await applyCurrentPageContentChanges([{
      blockId: action.blockId,
      beforeContent: block.content,
      afterContent: linked.doc,
    }]);
    clearBlockSelection();
    window.getSelection()?.removeAllRanges();
  }

  function selectedBlocksContainTask(): boolean {
    return contextMenuBlocks().some((block) => isTaskContent(block.content));
  }

  function selectedBlocksContainPlainBullet(): boolean {
    return contextMenuBlocks().some((block) => !isTaskContent(block.content));
  }

  async function convertSelectedBlocks(kind: "task-to-bullet" | "bullet-to-todo") {
    const selected = contextMenuBlocks();
    if (selected.length === 0) {
      selectionMenu = null;
      showSelectionCopyMessage("Block changed; select it again");
      return;
    }

    const updates: BlockContentChange[] = [];
    for (const block of selected) {
      const nextContent =
        kind === "task-to-bullet"
          ? taskToBulletContent(block.content)
          : bulletToTodoContent(block.content);
      if (nextContent !== block.content) {
        updates.push({
          blockId: block.id,
          beforeContent: block.content,
          afterContent: nextContent,
        });
      }
    }

    if (updates.length === 0) {
      selectionMenu = null;
      return;
    }

    await applyCurrentPageContentChanges(updates);
    promotedSelectionCopyText = null;
    selectionMenu = null;
  }

  function showSelectionCopyMessage(message: string) {
    selectionCopyMessage = message;
    if (selectionCopyTimer !== undefined) {
      window.clearTimeout(selectionCopyTimer);
    }
    selectionCopyTimer = window.setTimeout(() => {
      selectionCopyMessage = "";
      selectionCopyTimer = undefined;
    }, 1600);
  }

  async function writeClipboardText(text: string) {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
        return;
      }
    } catch {
      // WebKitGTK can reject the async Clipboard API even from a user gesture.
    }

    const scratch = document.createElement("textarea");
    scratch.value = text;
    scratch.style.position = "fixed";
    scratch.style.left = "-9999px";
    scratch.style.opacity = "0";
    document.body.appendChild(scratch);
    scratch.select();
    const copied = document.execCommand("copy");
    scratch.remove();
    if (!copied) {
      throw new Error("clipboard text API is unavailable");
    }
  }

  async function copySelectedBlocks() {
    const { markdown } = selectedClipboardPayload();
    if (!markdown) return;
    try {
      await writeClipboardText(markdown);
      selectionMenu = null;
      showSelectionCopyMessage("Copied");
    } catch (e) {
      showSelectionCopyMessage(e instanceof Error ? e.message : String(e));
    }
  }

  async function cutSelectedBlocks() {
    const { markdown } = selectedClipboardPayload();
    if (!markdown) return;
    try {
      await writeClipboardText(markdown);
      await handleDeleteSelected();
      showSelectionCopyMessage("Cut");
    } catch (e) {
      showSelectionCopyMessage(e instanceof Error ? e.message : String(e));
    }
  }

  function handleCopySelection(e: ClipboardEvent) {
    if (!canHandleBlockSelectionEvent(e)) return;
    const { markdown, plainText } = selectedClipboardPayload();
    if (!markdown || !e.clipboardData) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", plainText);
    e.clipboardData.setData("text/markdown", markdown);
    selectionMenu = null;
    showSelectionCopyMessage("Copied");
  }

  function handleCutSelection(e: ClipboardEvent) {
    if (!canHandleBlockSelectionEvent(e)) return;
    const { markdown, plainText } = selectedClipboardPayload();
    if (!markdown || !e.clipboardData) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", plainText);
    e.clipboardData.setData("text/markdown", markdown);
    selectionMenu = null;
    void (async () => {
      try {
        await handleDeleteSelected();
        showSelectionCopyMessage("Cut");
      } catch (error) {
        showSelectionCopyMessage(error instanceof Error ? error.message : String(error));
      }
    })();
  }

  function handleSelectionContextMenu(e: MouseEvent) {
    const target = e.target as Element | null;
    if (!pageContentEl?.contains(target)) return;
    if (target?.closest?.("input, textarea, select, .cm-editor, .app-context-menu, .image-size-menu, .make-link-menu")) {
      return;
    }

    const blockId = blockIdFromNode(target);
    const clickedBlock = blockId ? blockRenderState.blockById.get(blockId) : undefined;
    const makeLink = clickedBlock
      ? captureNativeSelectionMakeLinkAction() ?? cachedSelectionMakeLinkAction(blockId!)
      : null;
    const showPageActions = canRenamePage || canDeletePage;
    // Journal/backlink views can mount multiple PageContent instances, and
    // every one receives the same window-level contextmenu event. Only the
    // instance that owns the clicked target may prevent the native menu.
    if (!clickedBlock && !makeLink && !showPageActions) return;

    const currentSelectedBlocks = selectedBlocksInDocumentOrder();
    const selectionIncludesClickedBlock = !!blockId
      && selectedBlockIds.has(blockId)
      && currentSelectedBlocks.some((block) => block.id === blockId);
    const menuBlocks = selectionIncludesClickedBlock
      ? currentSelectedBlocks
      : clickedBlock
        ? [clickedBlock]
        : [];
    const menuBlockIds = menuBlocks.map((block) => block.id);

    if (menuBlocks.length === 0 && !makeLink && !showPageActions) return;

    e.preventDefault();
    e.stopPropagation();
    if (blockId && !selectionIncludesClickedBlock && clickedBlock) {
      selectedBlockIds = new Set([blockId]);
      claimBlockSelectionOwner();
      promotedSelectionCopyText = null;
    }
    selectionMenu = {
      ...contextMenuPositionFromEvent(e, {
        width: 230,
        height: (menuBlocks.length > 0 ? 92 : 12)
          + (makeLink ? 42 : 0)
          + (menuBlocks.some((block) => isTaskContent(block.content)) ? 42 : 0)
          + (menuBlocks.some((block) => !isTaskContent(block.content)) ? 42 : 0)
          + (showPageActions ? 84 : 0),
      }),
      blockIds: menuBlockIds,
      blockCount: menuBlocks.length,
      canTurnTasksToBullets: menuBlocks.some((block) => isTaskContent(block.content)),
      canTurnBulletsToTodos: menuBlocks.some((block) => !isTaskContent(block.content)),
      showPageActions,
      ...(makeLink ? { makeLink } : {}),
    };
  }

  function closeSelectionMenu() {
    selectionMenu = null;
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

    promotedSelectionCopyText = sel.toString().trim() || null;
    selectedBlockIds = next;
    claimBlockSelectionOwner();
    focusedBlockId = null;
    lastSelectionMakeLinkAction = null;
    sel.removeAllRanges();
    const active = document.activeElement as HTMLElement | null;
    if (active && (active.isContentEditable || active.closest(".cm-editor"))) {
      active.blur();
    }
  }

  function handleSelectionMouseUp() {
    // Defer so the browser has committed the final range for this drag.
    setTimeout(() => {
      captureNativeSelectionMakeLinkAction();
      promoteTextSelectionToBlocks();
    }, 0);
  }

  function blockIdAtPoint(x: number, y: number): string | null {
    const node = document.elementFromPoint(x, y);
    return blockIdFromNode(node);
  }

  function setDraggedBlockSelection(startId: string, endId: string) {
    const startIdx = blocks.findIndex((block) => block.id === startId);
    const endIdx = blocks.findIndex((block) => block.id === endId);
    if (startIdx === -1 || endIdx === -1) return;

    const [from, to] = startIdx < endIdx ? [startIdx, endIdx] : [endIdx, startIdx];
    const next = new Set<string>();
    for (let i = from; i <= to; i++) {
      next.add(blocks[i].id);
    }

    promotedSelectionCopyText = null;
    selectedBlockIds = next;
    claimBlockSelectionOwner();
    focusedBlockId = null;
    selectionMenu = null;
    lastSelectionMakeLinkAction = null;
    window.getSelection()?.removeAllRanges();

    const active = document.activeElement as HTMLElement | null;
    if (active && (active.isContentEditable || active.closest(".cm-editor"))) {
      active.blur();
    }
  }

  function handleBlockSelectionPointerDown(e: PointerEvent) {
    if (useUnifiedEditorPrototype || e.button !== 0) return;
    lastSelectionMakeLinkAction = null;
    const startBlockId = blockIdFromNode(e.target as Node | null);
    if (!startBlockId) return;
    blockSelectionDrag = {
      pointerId: e.pointerId,
      startBlockId,
      started: false,
      startX: e.clientX,
      startY: e.clientY,
    };
  }

  function handleBlockSelectionPointerMove(e: PointerEvent) {
    const drag = blockSelectionDrag;
    if (!drag || drag.pointerId !== e.pointerId || (e.buttons & 1) === 0) return;
    if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < 4) return;

    const endBlockId = blockIdAtPoint(e.clientX, e.clientY);
    if (!endBlockId || endBlockId === drag.startBlockId) return;

    drag.started = true;
    setDraggedBlockSelection(drag.startBlockId, endBlockId);
    e.preventDefault();
  }

  function handleBlockSelectionPointerUp(e: PointerEvent) {
    if (blockSelectionDrag?.pointerId !== e.pointerId) return;
    if (blockSelectionDrag.started) {
      e.preventDefault();
    }
    blockSelectionDrag = null;
  }

  function handleBlockSelectionPointerCancel(e: PointerEvent) {
    if (blockSelectionDrag?.pointerId === e.pointerId) {
      blockSelectionDrag = null;
    }
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
    if (!canHandleBlockSelectionEvent(e)) return;
    if (isTabKey(e)) {
      // Selection presence is the intent signal — no need to consult
      // document.activeElement. Multi-block Tab always takes precedence over
      // in-editor Tab (a stale editor focus from the last click would otherwise
      // let the browser move focus and clear the selection).
      e.preventDefault();
      void handleIndentSelection(e.shiftKey ? "out" : "in");
    } else if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "x") {
      e.preventDefault();
      void cutSelectedBlocks();
    } else if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      handleDeleteSelected();
    } else if (e.key === "Escape") {
      clearBlockSelection();
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

<svelte:window
  onkeydown={handleKeydownForSelection}
  onpointermove={handleBlockSelectionPointerMove}
  onpointerup={handleBlockSelectionPointerUp}
  onpointercancel={handleBlockSelectionPointerCancel}
  onmouseup={handleSelectionMouseUp}
  oncopy={handleCopySelection}
  oncut={handleCutSelection}
  oncontextmenu={handleSelectionContextMenu}
  onclick={closeSelectionMenu}
/>

<div class="page-content" bind:this={pageContentEl} class:compact class:bookPage={isImportedBookPage}>
  <div class="page-heading">
    <div class="page-title-row">
      {#if renamingTitle}
        <input
          class="page-title-input"
          bind:this={renameInputEl}
          bind:value={renameDraft}
          disabled={renameBusy}
          spellcheck="false"
          aria-label="Rename page"
          onkeydown={handleRenameKeydown}
        />
        <button class="rename-page-btn" type="button" onclick={() => void commitRename()} disabled={renameBusy}>
          Save
        </button>
        <button class="rename-page-btn" type="button" onclick={cancelRename} disabled={renameBusy}>
          Cancel
        </button>
      {:else}
        <h1 class="page-title" class:journal-date={page.is_journal}>{displayPageTitle}</h1>
        {#if canRenamePage}
          <button
            class="rename-page-btn icon"
            type="button"
            title="Rename page"
            aria-label="Rename page"
            onclick={startRename}
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
              <path d="M12 20h9" />
              <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" />
            </svg>
          </button>
        {/if}
        {#if canDeletePage}
          <button
            class="rename-page-btn icon danger"
            type="button"
            title="Delete page"
            aria-label="Delete page"
            onclick={() => void deleteCurrentPage()}
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
              <polyline points="3 6 5 6 21 6" />
              <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
              <path d="M10 11v6" />
              <path d="M14 11v6" />
              <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
            </svg>
          </button>
        {/if}
      {/if}
    </div>
    <div class="page-heading-actions">
      {#if !compact}
        <PageMenu
          {collectionStatus}
          {collectionKind}
          busy={collectionBusy}
          onSetCollection={updateCollection}
        />
        <button
          class="prototype-toggle"
          type="button"
          onclick={handleSuggestLinks}
          disabled={linkCandidatesLoading || conceptEdgeBusy || blocks.length === 0}
          title="Find reviewable link suggestions: exact page-title mentions and AI concept edges"
        >
          {suggestLinksButtonLabel}
        </button>
      {/if}
      <button
        class="prototype-toggle"
        type="button"
        onclick={() => setUnifiedEditorPrototype(!useUnifiedEditorPrototype)}
        title="Try the experimental one-surface editor for cross-block text selection on this page/day"
      >
        {useUnifiedEditorPrototype ? "Classic block editor" : "Experimental continuous editor"}
      </button>
    </div>
  </div>

  {#if renameError}
    <div class="load-error">{renameError}</div>
  {/if}

  {#if !compact && (conceptEdgeBusy || conceptEdgesError || conceptEdgesMessage)}
    <div class="concept-edge-status" class:error={!!conceptEdgesError} role="status">
      {conceptEdgesError || conceptEdgeProgress || conceptEdgesMessage || "Finding concept edges..."}
    </div>
  {/if}

  {#if !compact && collectionKind !== null}
    <CollectionMembers
      kind={collectionKind}
      members={collectionMembers}
      memberCount={collectionMemberCount}
      onNavigate={navigateToCollectionMember}
    />
  {/if}

  {#if loadError}
    <div class="load-error">
      Error: {loadError}
      <button type="button" onclick={() => void loadBlocks()}>Retry</button>
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
      <button class="selection-toolbar-btn" onclick={copySelectedBlocks} disabled={analyzingSelection}>
        Copy
      </button>
      <button class="selection-toolbar-btn danger" onclick={handleDeleteSelected} disabled={analyzingSelection}>
        Delete
      </button>
      <button
        class="selection-toolbar-btn"
        onclick={clearBlockSelection}
        disabled={analyzingSelection}
      >
        Clear
      </button>
      {#if selectionCopyMessage}
        <span class="selection-copy-status">{selectionCopyMessage}</span>
      {/if}
    </div>
    {#if analyzeSelectionError}
      <div class="selection-toolbar-error">{analyzeSelectionError}</div>
    {/if}
  {/if}

  {#if selectionMenu}
    <div
      class="selection-context-menu app-context-menu"
      style={`left: ${selectionMenu.x}px; top: ${selectionMenu.y}px;`}
      role="menu"
      tabindex="-1"
      onpointerdown={(e) => e.stopPropagation()}
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => { if (e.key === "Escape") selectionMenu = null; }}
    >
      {#if selectionMenu.makeLink}
        <button
          type="button"
          role="menuitem"
          onclick={() => void makeSelectionLink()}
        >
          Make link
        </button>
      {/if}
      {#if selectionMenu.canTurnTasksToBullets}
       <button
         type="button"
         role="menuitem"
         onclick={() => void convertSelectedBlocks("task-to-bullet")}
       >
         {selectionMenu.blockCount > 1 ? "Turn tasks into bullets" : "Turn task into bullet"}
       </button>
      {/if}
      {#if selectionMenu.canTurnBulletsToTodos}
       <button
         type="button"
         role="menuitem"
         onclick={() => void convertSelectedBlocks("bullet-to-todo")}
       >
         {selectionMenu.blockCount > 1 ? "Turn bullets into TODOs" : "Turn bullet into TODO"}
       </button>
      {/if}
      {#if selectionMenu.blockCount > 0}
        <button type="button" role="menuitem" onclick={copySelectedBlocks}>Copy selection</button>
        <button
          type="button"
          role="menuitem"
          onclick={clearBlockSelection}
        >
          Clear selection
        </button>
      {/if}
      {#if selectionMenu.showPageActions}
        {#if selectionMenu.blockCount > 0 || selectionMenu.makeLink}
          <div class="selection-menu-separator" role="separator"></div>
        {/if}
        {#if canRenamePage}
          <button
            type="button"
            role="menuitem"
            onclick={() => { selectionMenu = null; startRename(); }}
          >
            Rename page
          </button>
        {/if}
        {#if canDeletePage}
          <button
            type="button"
            role="menuitem"
            class="danger"
            onclick={() => void deleteCurrentPage()}
          >
            Delete page
          </button>
        {/if}
      {/if}
    </div>
  {/if}

  {#if !compact && (linkCandidatesRevealed || linkCandidates.length > 0 || linkCandidatesLoading || linkCandidatesError || lastLinkCandidateAction)}
    <div class="link-candidates-panel">
      <div class="link-candidates-head">
        <div>
          <div class="link-candidates-title">Suggested links</div>
          <div class="link-candidates-subtitle">Review exact page mentions and AI-discovered semantic concept edges before linking them.</div>
        </div>
        <div class="link-candidates-actions">
          <button
            class="link-candidates-refresh concept-edge-panel-button"
            type="button"
            onclick={handleCreateConceptEdges}
            disabled={conceptEdgeBusy || linkCandidatesLoading || blocks.length === 0}
            title="Use AI to extract semantic concepts into reviewable edge suggestions"
          >
            {conceptEdgeBusy ? "Finding AI concept edges..." : "Find AI concept edges"}
          </button>
          <button
            class="link-candidates-refresh"
            type="button"
            onclick={handleFindLinks}
            disabled={linkCandidatesLoading || conceptEdgeBusy}
            title="Scan this page for exact mentions of existing page titles"
          >
            {linkCandidatesLoading ? "Scanning exact mentions..." : "Scan exact mentions"}
          </button>
        </div>
      </div>

      {#if linkCandidatesError}
        <div class="link-candidates-error">{linkCandidatesError}</div>
      {/if}

      {#if lastLinkCandidateAction}
        <div class="link-candidates-undo">
          {lastLinkCandidateAction.kind === "accepted" ? "Linked" : "Dismissed"}
          {lastLinkCandidateAction.candidates.length}
          occurrence{lastLinkCandidateAction.candidates.length === 1 ? "" : "s"}.
          <button type="button" onclick={undoLastLinkCandidateAction}>Undo</button>
        </div>
      {/if}

      {#if linkCandidateGroups.length > 0}
        <div class="link-candidates-summary">
          Reviewing {linkCandidateOccurrenceTotal} occurrence{linkCandidateOccurrenceTotal === 1 ? "" : "s"} grouped into {linkCandidateGroups.length} concept{linkCandidateGroups.length === 1 ? "" : "s"}.
        </div>
        <div class="link-candidates-list">
          {#each linkCandidateGroups as group (group.key)}
            {@const context = linkCandidateContextPreview(group)}
            <div class="link-candidate-row">
              <span
                class="link-candidate-source"
                class:semantic={group.sources.includes("semantic_concept")}
              >
                {linkCandidateGroupSourceLabel(group)}
              </span>
              <div class="link-candidate-copy-wrap">
                <div class="link-candidate-copy">
                  <button
                    class="link-candidate-anchor"
                    type="button"
                    onclick={() => handleRevealLinkCandidateGroup(group)}
                    title="Jump to the first occurrence on this page"
                  >
                    {linkCandidateAnchorSummary(group)}
                  </button>
                  <span class="link-candidate-arrow">-&gt;</span>
                  <button
                    class="link-candidate-target"
                    type="button"
                    onclick={() => navigateToCandidateTarget(group.primary)}
                    title="Open suggested target page"
                  >
                    {group.primary.to_page_title}
                  </button>
                </div>
                <div class="link-candidate-meta">
                  {group.occurrenceCount} occurrence{group.occurrenceCount === 1 ? "" : "s"} on this page
                  {#if group.canFixSpelling}
                    <span class="link-candidate-correction">likely spelling fix</span>
                  {:else if group.hasCanonicalTarget}
                    <span class="link-candidate-correction">alias / canonical target</span>
                  {/if}
                </div>
                {#if context}
                  <button
                    class="link-candidate-context"
                    type="button"
                    onclick={() => revealLinkCandidateOccurrence(context.candidate)}
                    title="Jump to this occurrence"
                  >
                    <span class="link-candidate-context-label">{context.blockLabel}</span>
                    <span class="link-candidate-context-text">
                      {#if context.leadingEllipsis}<span class="link-candidate-ellipsis">...</span>{/if}
                      {#if context.before}<span>{context.before} </span>{/if}
                      <mark>{context.anchor}</mark>
                      {#if context.after}<span> {context.after}</span>{/if}
                      {#if context.trailingEllipsis}<span class="link-candidate-ellipsis">...</span>{/if}
                    </span>
                  </button>
                {/if}
              </div>
              <div class="link-candidate-actions">
                <button type="button" onclick={() => handleAcceptLinkCandidateGroup(group)}>
                  {group.hasCanonicalTarget ? "Fix + link" : group.occurrenceCount > 1 ? "Link all" : "Link"}
                </button>
                <button type="button" onclick={() => handleDismissLinkCandidateGroup(group)}>Not an edge</button>
              </div>
            </div>
          {/each}
        </div>
      {:else if linkCandidatesLoading}
        <div class="link-candidates-empty">Scanning for unlinked page mentions...</div>
      {:else if conceptEdgeBusy}
        <div class="link-candidates-empty">{conceptEdgeProgress || "Finding concept edges..."}</div>
      {:else if isImportedBookPage}
        <div class="link-candidates-empty">Automatic exact-title scanning is skipped for large book pages. Use AI concept edges for semantic suggestions, or scan exact mentions manually.</div>
      {:else if linkCandidatesRevealed}
        <div class="link-candidates-empty">No pending link suggestions.</div>
      {/if}
    </div>
  {/if}

  {#if useUnifiedEditorPrototype}
    <UnifiedPageEditor
      {page}
      {compact}
      onReload={reloadCurrentPageContent}
      onExitPrototype={() => setUnifiedEditorPrototype(false)}
    />
  {:else}
    <div class="blocks-container" bind:this={blocksViewportEl} onpointerdown={handleBlockSelectionPointerDown}>
      {#if virtualWindow.topSpacer > 0}
        <div class="virtual-spacer" style={`height: ${virtualWindow.topSpacer}px;`} aria-hidden="true"></div>
      {/if}
      {#each windowedBlocks as block (block.id)}
        {@const thread = threadById.get(block.id) ?? NO_THREAD}
        <div
          class="block-shell"
          class:revealed={revealedBlockId === block.id}
          id={`block-${block.id}`}
          data-block-id={block.id}
          use:trackBlockHeight={{ blockId: block.id, enabled: shouldVirtualizeBlocks }}
        >
          <BlockEditor
            bind:this={blockRefs[block.id]}
            {block}
            pageId={page.id}
            pageTitle={page.title}
            {assetBaseDir}
            bookMode={isImportedBookPage}
            guides={getBlockGuides(block.id)}
            threadElbow={thread.elbow}
            threadContinuationDepth={thread.continuationDepth}
            threadStem={thread.stem}
            showGuides={showBlockGuides}
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
            onAnchor={handleBlockAnchor}
            onIndent={handleIndent}
            onBulletClick={handleBulletClick}
            onPasteBlocks={handlePasteBlocks}
            onToggleCollapse={toggleCollapse}
            onContentChange={handleBlockContentChange}
          />
        </div>
      {/each}
      {#if shouldProgressivelyRenderBook && !renderedAllVisibleBlocks}
        <div class="book-progressive-loader" aria-live="polite">
          <span>Showing {visibleRenderedBlockCount} of {visibleBlocks.length} blocks</span>
          <button type="button" onclick={() => growProgressiveBookRenderWindow()}>
            Load more
          </button>
        </div>
      {/if}
      {#if virtualWindow.bottomSpacer > 0}
        <div class="virtual-spacer" style={`height: ${virtualWindow.bottomSpacer}px;`} aria-hidden="true"></div>
      {/if}
    </div>

    {#if !shouldProgressivelyRenderBook || renderedAllVisibleBlocks}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="click-below" onclick={handleClickBelow}></div>
    {/if}
  {/if}

  {#if showHierarchySection && (parentPage || childPages.length > 0)}
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

  {#if showBelowPageSections && backlinks.length > 0}
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
                  <div
                    class="backlink-content"
                    use:bionicReader={node.block.content}
                    use:hydrateRenderedMedia={node.block.content}
                  >
                    {@html renderBlock(node.block.content, bl.sourceAssetBaseDir)}
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

  .page-content.bookPage {
    padding-bottom: 48px;
  }

  .page-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }

  .page-title {
    font-size: 32px;
    font-weight: 700;
    margin: 0;
    color: var(--text-primary);
    min-width: 0;
    overflow-wrap: break-word;
    word-break: normal;
  }

  .page-title.journal-date {
    font-size: 22px;
    font-weight: 700;
    line-height: 1.2;
  }

  .page-title-input {
    flex: 1;
    min-width: 0;
    font-size: 22px;
    font-weight: 700;
    padding: 4px 8px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: var(--bg-input, var(--bg-secondary));
    color: var(--text-primary);
    outline: none;
  }

  .rename-page-btn {
    flex-shrink: 0;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text-secondary);
    font-size: 12px;
    cursor: pointer;
  }

  .rename-page-btn.icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    opacity: 0.55;
  }

  .rename-page-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--accent);
    background: var(--bg-hover);
    opacity: 1;
  }

  .rename-page-btn.icon.danger:hover:not(:disabled) {
    color: var(--danger);
    border-color: var(--danger);
  }

  .rename-page-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .page-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;
  }

  .page-content.bookPage .page-heading {
    max-width: 820px;
    margin: 0 auto 22px;
    padding-bottom: 14px;
    border-bottom: 1px solid var(--border);
    align-items: flex-start;
  }

  .page-content.bookPage .page-title {
    font-family: Georgia, "Times New Roman", serif;
    font-size: clamp(26px, 4vw, 42px);
    line-height: 1.12;
    text-align: center;
  }

  .page-heading-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .prototype-toggle {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 12px;
    padding: 5px 8px;
    flex-shrink: 0;
  }

  .prototype-toggle:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .prototype-toggle:disabled {
    cursor: default;
    opacity: 0.65;
  }

  .concept-edge-status {
    margin: -2px 0 10px;
    padding: 6px 8px;
    border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--border));
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 8%, transparent);
    color: var(--text-secondary);
    font-size: 12px;
  }

  .concept-edge-status.error {
    border-color: var(--danger);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
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

  .block-shell.revealed :global(.block-item) {
    box-shadow:
      0 0 0 2px color-mix(in srgb, var(--accent-yellow) 78%, transparent),
      0 0 18px color-mix(in srgb, var(--accent-yellow) 34%, transparent);
    animation: target-block-pulse 2.2s ease-out;
  }

  @keyframes target-block-pulse {
    0% {
      transform: translateX(-2px);
      box-shadow:
        0 0 0 3px color-mix(in srgb, var(--accent-yellow) 95%, transparent),
        0 0 26px color-mix(in srgb, var(--accent-yellow) 55%, transparent);
    }
    100% {
      transform: translateX(0);
      box-shadow:
        0 0 0 2px color-mix(in srgb, var(--accent-yellow) 78%, transparent),
        0 0 18px color-mix(in srgb, var(--accent-yellow) 34%, transparent);
    }
  }

  .page-content.bookPage .blocks-container {
    max-width: 760px;
    margin: 0 auto;
  }

  .load-error {
    margin-bottom: 16px;
    padding: 12px;
    border-radius: 8px;
    background: var(--danger-bg);
    color: var(--danger);
    font-family: monospace;
    font-size: 13px;
    white-space: pre-wrap;
  }

  .selection-toolbar {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--bg-secondary);
    border: 1px solid var(--accent);
    border-radius: 8px;
    padding: 8px 10px;
    margin-bottom: 8px;
  }

  .selection-count {
    font-size: 12px;
    color: var(--text-secondary);
    margin-right: 4px;
  }

  .selection-toolbar-btn {
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
    color: var(--text-primary);
    cursor: pointer;
  }

  .selection-toolbar-btn:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .selection-toolbar-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .selection-toolbar-btn.danger {
    color: var(--danger);
  }

  .selection-copy-status {
    font-size: 12px;
    color: var(--text-secondary);
  }

  .selection-context-menu {
    position: fixed;
    z-index: 2147483000;
    min-width: 150px;
    padding: 4px;
  }

  .selection-context-menu button {
    display: block;
    width: 100%;
    padding: 6px 8px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    font-size: 12px;
    text-align: left;
    cursor: pointer;
  }

  .selection-context-menu button:hover {
    background: var(--bg-tertiary);
  }

  .selection-context-menu button.danger {
    color: var(--danger);
  }

  .selection-menu-separator {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }

  .selection-toolbar-error {
    font-size: 12px;
    color: var(--danger);
    margin-bottom: 8px;
  }

  .link-candidates-panel {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent-cyan) 8%, var(--bg-secondary));
    margin: 8px 0 12px;
    padding: 10px;
  }

  .link-candidates-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .link-candidates-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .link-candidates-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .link-candidates-subtitle,
  .link-candidates-empty,
  .link-candidates-summary,
  .link-candidate-meta {
    font-size: 12px;
    color: var(--text-secondary);
  }

  .link-candidates-refresh,
  .link-candidate-actions button,
  .link-candidates-undo button {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    cursor: pointer;
    font-size: 12px;
    padding: 4px 8px;
  }

  .link-candidates-refresh:hover:not(:disabled),
  .link-candidate-actions button:hover,
  .link-candidates-undo button:hover {
    border-color: var(--accent);
  }

  .link-candidates-refresh:disabled {
    opacity: 0.65;
    cursor: default;
  }

  .concept-edge-panel-button {
    border-color: color-mix(in srgb, var(--accent) 60%, var(--border));
    color: var(--accent);
  }

  .link-candidates-error {
    margin-top: 8px;
    color: var(--danger);
    font-size: 12px;
  }

  .link-candidates-undo {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .link-candidates-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 10px;
    max-height: min(420px, 45vh);
    overflow-y: auto;
    padding-right: 4px;
  }

  .link-candidate-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 8px;
    border-radius: 6px;
    background: var(--bg-primary);
  }

  .link-candidate-source {
    flex-shrink: 0;
    padding: 2px 6px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--text-muted);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
  }

  .link-candidate-source.semantic {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--border));
    color: var(--accent);
  }

  .link-candidate-copy {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: 12px;
  }

  .link-candidate-copy-wrap {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  .link-candidate-anchor {
    border: none;
    background: none;
    color: var(--text-primary);
    cursor: pointer;
    padding: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 220px;
    font: inherit;
    text-align: left;
  }

  .link-candidate-anchor:hover,
  .link-candidate-anchor:focus-visible {
    color: var(--text-link);
    text-decoration: underline;
  }

  .link-candidate-arrow {
    color: var(--text-muted);
  }

  .link-candidate-correction {
    margin-left: 8px;
    color: var(--accent);
    font-weight: 600;
  }

  .link-candidate-context {
    display: flex;
    align-items: baseline;
    gap: 6px;
    width: 100%;
    min-width: 0;
    border: 1px solid color-mix(in srgb, var(--border) 72%, transparent);
    border-radius: 5px;
    background: color-mix(in srgb, var(--bg-secondary) 58%, transparent);
    color: var(--text-secondary);
    cursor: pointer;
    padding: 4px 6px;
    font-size: 11px;
    line-height: 1.35;
    text-align: left;
  }

  .link-candidate-context:hover,
  .link-candidate-context:focus-visible {
    border-color: color-mix(in srgb, var(--accent) 48%, var(--border));
    color: var(--text-primary);
  }

  .link-candidate-context-label {
    flex-shrink: 0;
    color: var(--text-muted);
    font-weight: 600;
  }

  .link-candidate-context-text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .link-candidate-context mark {
    border-radius: 3px;
    background: color-mix(in srgb, var(--accent-yellow) 36%, transparent);
    color: inherit;
    padding: 0 2px;
    box-shadow: inset 0 -2px 0 var(--accent-yellow);
  }

  .link-candidate-ellipsis {
    color: var(--text-muted);
  }

  .link-candidate-target {
    border: none;
    background: none;
    color: var(--text-link);
    cursor: pointer;
    padding: 0;
    font-size: 12px;
  }

  .link-candidate-target:hover {
    text-decoration: underline;
  }

  .link-candidate-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .link-candidates-empty,
  .link-candidates-summary {
    margin-top: 8px;
  }

  .block-shell {
    position: relative;
    z-index: 0;
    padding-bottom: 0;
    overflow: visible;
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

  .page-content.bookPage .block-shell {
    padding-bottom: 0;
  }

  .block-shell.image-menu-shell,
  .block-shell:has(:global(.image-menu-open)) {
    z-index: 3000;
    contain: none;
    overflow: visible;
  }

  .virtual-spacer {
    width: 100%;
    pointer-events: none;
  }

  .book-progressive-loader {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    color: var(--text-secondary);
    font-size: 12px;
    padding: 18px 0 24px;
  }

  .book-progressive-loader button {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    cursor: pointer;
    font-size: 12px;
    padding: 5px 10px;
  }

  .book-progressive-loader button:hover {
    border-color: var(--accent);
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
  }

  .compact .page-heading {
    margin-bottom: 4px;
  }

  .compact .prototype-toggle {
    font-size: 11px;
    padding: 3px 6px;
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
    border: 1px solid var(--border);
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

  /* Phone: keep the title on its own row so action buttons cannot squeeze
     "math" (and journal dates) into one character per line. Desktop heading
     stays a single row. */
  @media (max-width: 640px) {
    .page-heading {
      flex-direction: column;
      align-items: stretch;
      gap: 8px;
    }

    .page-title-row {
      flex: none;
      width: 100%;
    }

    .page-title {
      font-size: 1.6rem;
      line-height: 1.2;
      overflow-wrap: break-word;
      word-break: keep-all;
    }

    .page-heading-actions {
      flex-shrink: 1;
      flex-wrap: wrap;
      width: 100%;
    }
  }
</style>
