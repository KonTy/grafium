<script lang="ts">
  import { tick } from "svelte";
  import { SvelteMap } from "svelte/reactivity";
  import PageTree from "./PageTree.svelte";
  import {
    countPages,
    listPagesWindow,
    createPage,
    bulkRenamePages,
    deletePage,
    deleteNamespace,
    deleteBookFolder,
    renamePage,
    getChildPages,
    getGraphInfo,
    openBookFolderInFileBrowser,
    openNamespaceInFileBrowser,
    openPageInFileBrowser,
  } from "../lib/api";
  import {
    getPageTree,
    toPageTreeView,
    withMissingCommandFallback,
    type PageTreeSource,
  } from "../lib/pageTree";
  import {
    ALL_PAGES_TREE_STORAGE_KEY,
    ALL_PAGES_SORT_STORAGE_KEY,
    ALL_PAGES_KIND_STORAGE_KEY,
    parsePageKindFilter,
    graphScopedKey,
    filterTreeByQuery,
    countTreePages,
    sortTree,
    type PageTreeViewNode,
    type PageKindFilter,
  } from "../lib/pageTreeState";
  import type { BulkRenameResult, Page } from "../lib/api";

  interface PageActionMenu {
    x: number;
    y: number;
    pageId: string | null;
    title: string;
    bookTitle: string | null;
    folder: boolean;
  }

  interface ConfirmDialog {
    message: string;
    confirmLabel: string;
  }

  interface RenameDialog {
    title: string;
    draft: string;
    pageId: string | null;
    folder: boolean;
  }

  interface Props {
    onNavigate: (title: string) => void;
    onPageDeleted?: () => void;
  }

  let { onNavigate, onPageDeleted }: Props = $props();

  // Fixed-height virtual list tuned for millions of rows: only the rows in (or
  // near) the viewport are ever in the DOM, and each window is fetched from the
  // DB on demand via offset paging off a partial index (~20ms even at offset
  // 900k). Nothing is capped — the full data set is browsable.
  const ROW_H = 44; // px per row, must match .page-row height
  const CHUNK = 200; // rows fetched per DB request
  const OVERSCAN = 8; // extra rows rendered above/below the viewport

  let total = $state(0);
  let sortByTitle = $state(false); // false = Recent (updated_at), true = A-Z (title)
  let kindFilter = $state<PageKindFilter>("all");
  let newPageTitle = $state("");
  let viewMode = $state<"tree" | "list">("tree");
  let treeSource = $state<PageTreeSource>("namespace");
  let pageTree: PageTreeViewNode[] = $state([]);
  let pageTreeAvailable: boolean | null = $state(null);
  let pageTreeLoading = $state(false);
  let pageTreeError = $state("");
  let pageTreeRequest = 0;

  /// Free-text filter over the tree.
  ///
  /// Scoped to the tree deliberately: the list view is virtualized, fetching
  /// only the rows around the viewport, so filtering it client-side would
  /// silently search a fraction of the graph and report "no matches" for pages
  /// that exist. The tree holds every page, so filtering it is a real search.
  let filterQuery = $state("");
  let bulkFrom = $state("");
  let bulkTo = $state("");
  let bulkBusy = $state(false);
  let bulkError = $state("");
  let bulkPreview: BulkRenameResult | null = $state(null);
  // Sorted first, then filtered. Filtering preserves order, so ordering the
  // whole tree once per sort change beats re-sorting the filtered result on
  // every keystroke — the sort is the expensive half.
  let sortedTree = $derived(sortTree(pageTree, sortByTitle ? "name" : "recent"));
  let visibleTree = $derived(filterTreeByQuery(sortedTree, filterQuery));
  let filteredCount = $derived(countTreePages(visibleTree));

  // Loaded rows keyed by absolute index; SvelteMap is reactive so the template
  // updates as windows stream in.
  let rows = new SvelteMap<number, Page>();
  const requested = new Set<number>(); // chunk indices already fetched
  let reloadToken = $state(0); // bump to force a re-fetch of the visible window

  let spacerEl: HTMLDivElement | null = $state(null);
  let actionMenuEl: HTMLDivElement | null = $state(null);
  let actionMenu: PageActionMenu | null = $state(null);
  let confirmDialog: ConfirmDialog | null = $state(null);
  let confirmResolver: ((ok: boolean) => void) | null = null;
  let renameDialog: RenameDialog | null = $state(null);
  let renameInputEl: HTMLInputElement | null = $state(null);
  let renameBusy = $state(false);
  let renameError = $state("");
  let relTop = $state(0); // px of list scrolled above the viewport top
  let visH = $state(0); // viewport height in px

  let startIndex = $derived(Math.max(0, Math.floor(relTop / ROW_H) - OVERSCAN));
  let endIndex = $derived(
    Math.min(total, Math.ceil((relTop + visH) / ROW_H) + OVERSCAN)
  );
  let visible = $derived.by(() => {
    const out: number[] = [];
    for (let i = startIndex; i < endIndex; i++) out.push(i);
    return out;
  });

  $effect(() => {
    void refreshCount();
  });

  /// Storage keys are scoped to the open graph — an expansion path is only
  /// meaningful inside the graph it came from.
  let graphPath: string | null = $state(null);
  /// Whether we have heard back about which graph is open at all.
  ///
  /// Distinct from `graphPath === null`, which is a real answer meaning "no
  /// path". Until this flips, the scoped key is not yet knowable and anything
  /// keyed on it would be reading and writing an unscoped key shared by every
  /// graph.
  let graphResolved = $state(false);
  $effect(() => {
    void getGraphInfo()
      .then((info) => { graphPath = info.path; })
      .catch(() => { graphPath = null; })
      .finally(() => { graphResolved = true; });
  });

  // Restored per graph: the order you browse in is a lasting preference, and
  // resetting it on every launch would undo the choice each time.
  let sortStorageKey = $derived(graphScopedKey(ALL_PAGES_SORT_STORAGE_KEY, graphPath));
  let restoredSortFor: string | null = $state(null);
  $effect(() => {
    // Nothing before the graph is known: restoring against the unscoped key
    // would apply one graph's preference to another, and would then be
    // overwritten a moment later when the real key arrives — silently
    // reverting a choice made in between.
    if (!graphResolved) return;
    const key = sortStorageKey;
    if (restoredSortFor === key) return;
    restoredSortFor = key;
    try {
      const saved = window.localStorage.getItem(key);
      const restored = saved === "name";
      if ((saved === "name" || saved === "recent") && restored !== sortByTitle) {
        sortByTitle = restored;
        // Any rows already fetched came back in the other order.
        resetWindows();
      }
    } catch {
      // Ignore an unreadable store and keep the default.
    }
  });

  // Same reasoning as the sort preference above: which kinds of page you want
  // to see is a lasting choice, and it is scoped per graph because one graph
  // may be all files while another is mostly link placeholders.
  let kindStorageKey = $derived(graphScopedKey(ALL_PAGES_KIND_STORAGE_KEY, graphPath));
  let restoredKindFor: string | null = $state(null);
  $effect(() => {
    if (!graphResolved) return;
    const key = kindStorageKey;
    if (restoredKindFor === key) return;
    restoredKindFor = key;
    try {
      const restored = parsePageKindFilter(window.localStorage.getItem(key));
      if (restored !== kindFilter) {
        kindFilter = restored;
        // The rows already fetched were a different set of pages, and the
        // total they were sized against counted different rows too.
        resetWindows();
        void refreshCount();
      }
    } catch {
      // Ignore an unreadable store and keep the default.
    }
  });

  $effect(() => {
    const source = treeSource;
    const filter = kindFilter;
    if (viewMode !== "tree") return;
    void loadPageTree(source, filter);
  });

  // The sidebar listens for this too. All Pages currently reloads on mount and
  // handles its own create/delete, so today it is always fresh — but it only
  // *dispatches* the event, and would silently show a stale tree the moment it
  // stays mounted beside an editor. Listening costs nothing and removes the
  // trap rather than leaving it for whoever changes the routing.
  $effect(() => {
    const refreshTree = () => {
      if (viewMode === "tree" && pageTreeAvailable !== false) void loadPageTree(treeSource);
    };
    window.addEventListener("page-tree-refresh", refreshTree);
    return () => window.removeEventListener("page-tree-refresh", refreshTree);
  });

  $effect(() => {
    if (!actionMenu) return;
    const closeOutside = (event: PointerEvent) => {
      if (!actionMenuEl?.contains(event.target as Node)) actionMenu = null;
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") actionMenu = null;
    };
    window.addEventListener("pointerdown", closeOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  });


  $effect(() => {
    if (!confirmDialog && !renameDialog) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (confirmDialog) answerConfirm(false);
      else if (renameDialog && !renameBusy) renameDialog = null;
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  });

  // Track scroll/resize of the enclosing .main-content scroller.
  $effect(() => {
    if (!spacerEl) return;
    const parent = spacerEl.closest(".main-content") as HTMLElement | null;
    if (!parent) return;
    const update = () => {
      const p = parent.getBoundingClientRect();
      const s = spacerEl!.getBoundingClientRect();
      relTop = p.top - s.top;
      visH = parent.clientHeight;
    };
    update();
    parent.addEventListener("scroll", update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(parent);
    return () => {
      parent.removeEventListener("scroll", update);
      ro.disconnect();
    };
  });

  // Fetch whatever windows the visible range needs.
  $effect(() => {
    reloadToken;
    startIndex;
    endIndex;
    sortByTitle;
    kindFilter;
    ensureVisibleLoaded();
  });

  function ensureVisibleLoaded() {
    if (endIndex <= startIndex) return;
    const byTitle = sortByTitle;
    const filter = kindFilter;
    const firstChunk = Math.floor(startIndex / CHUNK);
    const lastChunk = Math.floor((endIndex - 1) / CHUNK);
    for (let ch = firstChunk; ch <= lastChunk; ch++) {
      if (requested.has(ch)) continue;
      requested.add(ch);
      void fetchChunk(ch, byTitle, filter);
    }
  }

  async function fetchChunk(chunk: number, byTitle: boolean, filter: PageKindFilter) {
    const offset = chunk * CHUNK;
    try {
      const pages = await listPagesWindow(CHUNK, offset, byTitle, filter);
      // Ignore late responses from a superseded sort or filter: they describe
      // a window that no longer exists, and writing them in would interleave
      // two different result sets at the same row indices.
      if (byTitle !== sortByTitle || filter !== kindFilter) return;
      for (let k = 0; k < pages.length; k++) rows.set(offset + k, pages[k]);
    } catch (e) {
      requested.delete(chunk); // allow a retry
      console.error("Failed to load page window:", e);
    }
  }

  async function refreshCount() {
    try {
      const filter = kindFilter;
      const counted = await countPages(filter);
      // A count that arrives after the filter moved on would size the
      // scrollbar for the wrong result set.
      if (filter !== kindFilter) return;
      total = counted;
    } catch (e) {
      console.error("Failed to count pages:", e);
    }
  }

  async function loadPageTree(source: PageTreeSource, filter: PageKindFilter = kindFilter) {
    const request = ++pageTreeRequest;
    pageTreeLoading = true;
    pageTreeError = "";
    pageTree = [];
    try {
      const result = await withMissingCommandFallback(
        () => getPageTree(source, filter),
        [],
      );
      if (request !== pageTreeRequest || source !== treeSource) return;
      pageTreeAvailable = result.available;
      pageTree = result.available ? toPageTreeView(result.value, source) : [];
      if (!result.available) viewMode = "list";
    } catch (error) {
      if (request !== pageTreeRequest || source !== treeSource) return;
      pageTreeAvailable = true;
      pageTree = [];
      pageTreeError = String(error);
      console.warn(`[page-tree] Failed to load ${source} tree:`, error);
    } finally {
      if (request === pageTreeRequest) pageTreeLoading = false;
    }
  }

  function resetWindows() {
    rows.clear();
    requested.clear();
    reloadToken++;
  }

  // Changing the filter changes how many rows exist — often by a lot, since
  // most graphs have far more placeholders than files. Staying at the old
  // offset would drop you past the end of the shorter list, so go back to the
  // top. Sorting deliberately does not do this: the row count is unchanged
  // there, and keeping your place is the useful behaviour.
  function scrollListToTop() {
    const parent = spacerEl?.closest(".main-content") as HTMLElement | null;
    if (parent) parent.scrollTop = 0;
  }

  function setKindFilter(filter: PageKindFilter) {
    if (filter === kindFilter) return;
    kindFilter = filter;
    resetWindows();
    scrollListToTop();
    void refreshCount();
    try {
      window.localStorage.setItem(kindStorageKey, filter);
    } catch {
      // A full or disabled store only costs the preference, not the filter.
    }
  }

  function setSort(byTitle: boolean) {
    if (byTitle === sortByTitle) return;
    sortByTitle = byTitle;
    resetWindows();
    try {
      window.localStorage.setItem(sortStorageKey, byTitle ? "name" : "recent");
    } catch {
      // A full or disabled store only costs the preference, not the sort.
    }
  }

  async function previewBulkRename() {
    const from = bulkFrom.trim();
    bulkError = "";
    bulkPreview = null;
    if (!from) {
      bulkError = "Enter the text to find, e.g. self/";
      return;
    }
    bulkBusy = true;
    try {
      bulkPreview = await bulkRenamePages(from, bulkTo, true);
    } catch (e) {
      bulkError = errorMessage(e);
    } finally {
      bulkBusy = false;
    }
  }

  async function applyBulkRename() {
    const from = bulkFrom.trim();
    if (!from) return;
    let preview = bulkPreview;
    if (!preview) {
      await previewBulkRename();
      preview = bulkPreview;
    }
    if (!preview) return;
    const mergedCount = preview.merged?.length ?? 0;
    const changeCount = preview.renamed.length + mergedCount;
    if (changeCount === 0) return;
    const parts = [];
    if (preview.renamed.length > 0) {
      parts.push(`rename ${preview.renamed.length} page${preview.renamed.length === 1 ? "" : "s"}`);
    }
    if (mergedCount > 0) {
      parts.push(`merge ${mergedCount} into existing page${mergedCount === 1 ? "" : "s"}`);
    }
    const confirmed = await askConfirm(
      `${parts.join(" and ")}? This updates titles, markdown files, and [[wiki links]].`,
      "Rename",
    );
    if (!confirmed) return;
    bulkBusy = true;
    bulkError = "";
    try {
      const result = await bulkRenamePages(from, bulkTo, false);
      bulkPreview = result;
      resetWindows();
      await refreshCount();
      if (pageTreeAvailable !== false) void loadPageTree(treeSource);
      window.dispatchEvent(new CustomEvent("page-tree-refresh"));
    } catch (e) {
      bulkError = errorMessage(e);
    } finally {
      bulkBusy = false;
    }
  }

  async function handleCreatePage() {
    const title = newPageTitle.trim();
    if (!title) return;
    await createPage(title);
    newPageTitle = "";
    resetWindows();
    await refreshCount();
    if (pageTreeAvailable !== false) void loadPageTree(treeSource);
    window.dispatchEvent(new CustomEvent("page-tree-refresh"));
  }

  async function deletePageById(pageId: string, title: string) {
    // Deleting a page also removes namespaced subpages, unused media, the .md
    // file, and evicts it from favorites / recent pages.
    let childCount = 0;
    try {
      childCount = (await getChildPages(title)).length;
    } catch {
      childCount = 0;
    }
    const extra = childCount > 0
      ? ` and ${childCount} subpage${childCount === 1 ? "" : "s"}`
      : "";
    const confirmed = await askConfirm(
      `Delete page '${title}'${extra}? This removes notes, markdown files, `
      + `and media that nothing else uses. This cannot be undone.`,
    );
    if (!confirmed) return;

    try {
      await deletePage(pageId);
    } catch (e) {
      console.error("Failed to delete page:", e);
      alert(`Failed to delete page: ${errorMessage(e)}`);
      return;
    }
    await refreshAfterDeletion();
  }

  async function deleteFolderByTitle(title: string, pageId: string | null) {
    let childCount = 0;
    try {
      childCount = (await getChildPages(title)).length;
    } catch {
      childCount = 0;
    }
    const total = childCount + (pageId ? 1 : 0);
    const confirmed = await askConfirm(
      `Delete folder '${title}' and ${total} page${total === 1 ? "" : "s"} under it? `
      + `This removes notes, markdown files, and media that nothing else uses. `
      + `This cannot be undone.`,
    );
    if (!confirmed) return;

    try {
      if (pageId) await deletePage(pageId);
      else await deleteNamespace(title);
    } catch (e) {
      console.error("Failed to delete folder:", e);
      alert(`Failed to delete folder: ${errorMessage(e)}`);
      return;
    }
    await refreshAfterDeletion();
  }

  function titleFromTreeNode(node: PageTreeViewNode): string {
    if (node.page_title) return node.page_title;
    const prefix = `${treeSource}:`;
    return node.id.startsWith(prefix) ? node.id.slice(prefix.length) : node.label;
  }

  function bookTitleFromAnyTitle(title: string | null): string | null {
    if (!title) return null;
    const parts = title.split("/").filter(Boolean);
    if (parts[0] !== "Books" || !parts[1]) return null;
    return `Books/${parts[1]}`;
  }

  function askConfirm(message: string, confirmLabel = "Delete"): Promise<boolean> {
    confirmResolver?.(false);
    return new Promise((resolve) => {
      confirmResolver = resolve;
      confirmDialog = { message, confirmLabel };
    });
  }

  function answerConfirm(ok: boolean) {
    const resolve = confirmResolver;
    confirmResolver = null;
    confirmDialog = null;
    resolve?.(ok);
  }

  function errorMessage(error: unknown): string {
    if (error instanceof Error) return error.message;
    return String(error);
  }

  function removeBookBranchFromTree(nodes: PageTreeViewNode[], bookTitle: string): PageTreeViewNode[] {
    const prefix = `${bookTitle}/`;
    const prune = (node: PageTreeViewNode): PageTreeViewNode | null => {
      const title = titleFromTreeNode(node);
      if (title === bookTitle || title.startsWith(prefix)) return null;

      const children = node.children
        .map(prune)
        .filter((child): child is PageTreeViewNode => child !== null);
      if (!node.page_id && children.length === 0) return null;

      const selfCount = node.page_id ? 1 : 0;
      const count = children.reduce((sum, child) => sum + child.count, selfCount);
      const updated_at = Math.max(
        node.page_id ? node.updated_at : 0,
        ...children.map((child) => child.updated_at),
      );
      return { ...node, children, count, updated_at };
    };

    return nodes
      .map(prune)
      .filter((node): node is PageTreeViewNode => node !== null);
  }

  async function refreshAfterDeletion() {
    resetWindows();
    await refreshCount();
    if (pageTreeAvailable !== false) await loadPageTree(treeSource);
    window.dispatchEvent(new CustomEvent("page-tree-refresh"));
    // Let App.svelte drop deleted pages from the sidebar's recent list so stale
    // entries don't linger until the next currentPage change.
    onPageDeleted?.();
  }

  function menuPosition(event: MouseEvent): { x: number; y: number } {
    const rect = (event.currentTarget as HTMLElement | null)?.getBoundingClientRect();
    const rawX = rect ? rect.right - 224 : event.clientX;
    const rawY = rect ? rect.bottom + 4 : event.clientY;
    return {
      x: Math.min(Math.max(8, rawX), Math.max(8, window.innerWidth - 240)),
      y: Math.min(Math.max(8, rawY), Math.max(8, window.innerHeight - 180)),
    };
  }

  function openActionMenu(event: MouseEvent, target: Omit<PageActionMenu, "x" | "y">) {
    const { x, y } = menuPosition(event);
    actionMenu = { ...target, x, y };
  }

  function openMenuForNode(node: PageTreeViewNode, x: number, y: number) {
    if (treeSource !== "namespace" && !node.page_id) return;
    const title = titleFromTreeNode(node);
    actionMenu = {
      pageId: node.page_id,
      title,
      bookTitle: bookTitleFromAnyTitle(title),
      folder: treeSource === "namespace"
        && (!node.page_id || node.count > 1 || node.children.length > 0),
      x: Math.min(Math.max(8, x), Math.max(8, window.innerWidth - 240)),
      y: Math.min(Math.max(8, y), Math.max(8, window.innerHeight - 180)),
    };
  }

  function handleTreeNodeMenu(event: MouseEvent, node: PageTreeViewNode) {
    const { x, y } = event.type === "contextmenu"
      ? { x: event.clientX, y: event.clientY }
      : menuPosition(event);
    openMenuForNode(node, x, y);
  }

  function handleListPageMenu(event: MouseEvent, page: Page) {
    openActionMenu(event, {
      pageId: page.id,
      title: page.title,
      bookTitle: bookTitleFromAnyTitle(page.title),
      folder: false,
    });
  }

  async function handleRenameActionTarget() {
    const target = actionMenu;
    if (!target || target.bookTitle) return;
    actionMenu = null;
    renameError = "";
    renameBusy = false;
    renameDialog = {
      title: target.title,
      draft: target.title,
      pageId: target.pageId,
      folder: target.folder,
    };
    await tick();
    renameInputEl?.select();
  }

  async function commitRenameDialog() {
    if (!renameDialog || renameBusy) return;
    const current = renameDialog;
    const next = current.draft.trim().replace(/\/+$/, "");
    if (!next) {
      renameError = "Title cannot be empty";
      return;
    }
    if (next === current.title) {
      renameDialog = null;
      return;
    }
    renameBusy = true;
    renameError = "";
    try {
      if (current.folder) {
        await bulkRenamePages(`${current.title}/`, `${next}/`, false);
      }
      if (current.pageId) {
        await renamePage(current.pageId, next);
      }
      renameDialog = null;
      resetWindows();
      await refreshCount();
      if (pageTreeAvailable !== false) void loadPageTree(treeSource);
      window.dispatchEvent(new CustomEvent("page-tree-refresh"));
    } catch (e) {
      renameError = errorMessage(e);
    } finally {
      renameBusy = false;
    }
  }

  async function handleOpenInFileBrowser() {
    const target = actionMenu;
    if (!target) return;
    actionMenu = null;
    try {
      if (target.bookTitle) {
        await openBookFolderInFileBrowser(target.bookTitle);
      } else if (target.folder) {
        await openNamespaceInFileBrowser(target.title);
      } else if (target.pageId) {
        await openPageInFileBrowser(target.pageId);
      }
    } catch (e) {
      console.error("Failed to open in file browser:", e);
      alert(`Failed to open in file browser: ${errorMessage(e)}`);
    }
  }

  async function handleDeleteActionTarget() {
    const target = actionMenu;
    if (!target) return;
    actionMenu = null;

    if (target.bookTitle) {
      const confirmed = await askConfirm(
        `Delete imported book '${target.bookTitle}'? This removes the whole folder under pages/Books, including generated Markdown and assets. Original source files outside Grafium are not touched. This cannot be undone.`,
      );
      if (!confirmed) return;
      try {
        await deleteBookFolder(target.bookTitle);
      } catch (e) {
        console.error("Failed to delete book folder:", e);
        alert(`Failed to delete book folder: ${errorMessage(e)}`);
        return;
      }
      pageTree = removeBookBranchFromTree(pageTree, target.bookTitle);
      await refreshAfterDeletion();
      return;
    }

    if (target.folder) {
      await deleteFolderByTitle(target.title, target.pageId);
      return;
    }

    if (target.pageId) {
      await deletePageById(target.pageId, target.title);
    } else {
      await deleteFolderByTitle(target.title, null);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") handleCreatePage();
  }

  function fmtDate(ts: number | string): string {
    const value = typeof ts === "number" ? ts : Number(ts);
    if (!Number.isFinite(value) || value <= 0) return "—";
    const milliseconds = value < 1e12 ? value * 1000 : value;
    const date = new Date(milliseconds);
    return Number.isNaN(date.getTime()) ? "—" : date.toLocaleDateString();
  }
</script>

<div class="all-pages">
  <div class="header">
    <h1 class="page-title">All Pages</h1>
    <span class="count">{total.toLocaleString()} pages</span>
  </div>

  <div class="controls">
    <div class="new-page">
      <input
        type="text"
        placeholder="New page title..."
        bind:value={newPageTitle}
        onkeydown={handleKeydown}
        class="new-page-input"
      />
      <button onclick={handleCreatePage} class="btn-create">Create</button>
    </div>
    <button onclick={() => onNavigate("__import_media__")} class="btn-import-media" title="Import from video/audio (URL or file)">
      Import Media
    </button>
  </div>

  <div class="bulk-rename">
    <span class="bulk-rename-label">Bulk rename</span>
    <input
      class="new-page-input"
      type="text"
      placeholder="Find (e.g. self/)"
      bind:value={bulkFrom}
      disabled={bulkBusy}
      oninput={() => { bulkPreview = null; bulkError = ""; }}
    />
    <span class="bulk-rename-arrow" aria-hidden="true">→</span>
    <input
      class="new-page-input"
      type="text"
      placeholder="Replace with (empty ok)"
      bind:value={bulkTo}
      disabled={bulkBusy}
      oninput={() => { bulkPreview = null; bulkError = ""; }}
    />
    <button class="btn-import-media" type="button" onclick={() => void previewBulkRename()} disabled={bulkBusy}>
      Preview
    </button>
    <button
      class="btn-create"
      type="button"
      onclick={() => void applyBulkRename()}
      disabled={bulkBusy || !bulkFrom.trim() || (bulkPreview !== null && bulkPreview.renamed.length + (bulkPreview.merged?.length ?? 0) === 0)}
    >
      Rename{bulkPreview && bulkPreview.renamed.length + (bulkPreview.merged?.length ?? 0) > 0 ? ` ${bulkPreview.renamed.length + (bulkPreview.merged?.length ?? 0)}` : ""}
    </button>
  </div>
  {#if bulkError}
    <p class="bulk-rename-status error">{bulkError}</p>
  {:else if bulkPreview}
    <p class="bulk-rename-status">
      {bulkPreview.renamed.length} page{bulkPreview.renamed.length === 1 ? "" : "s"}
      {#if (bulkPreview.merged?.length ?? 0) > 0}
        · {bulkPreview.merged.length} merged
      {/if}
      {#if bulkPreview.links_updated > 0}
        · {bulkPreview.links_updated} block{bulkPreview.links_updated === 1 ? "" : "s"} with links updated
      {/if}
      {#if bulkPreview.skipped.length > 0}
        · {bulkPreview.skipped.length} skipped
      {/if}
    </p>
    {#if bulkPreview.renamed.length > 0}
      <ul class="bulk-rename-examples">
        {#each bulkPreview.renamed.slice(0, 8) as item (item.id)}
          <li><code>{item.old_title}</code> → <code>{item.new_title}</code></li>
        {/each}
        {#if bulkPreview.renamed.length > 8}
          <li>…and {bulkPreview.renamed.length - 8} more</li>
        {/if}
      </ul>
    {/if}
    {#if (bulkPreview.merged?.length ?? 0) > 0}
      <ul class="bulk-rename-examples">
        {#each bulkPreview.merged.slice(0, 8) as item (item.source_id)}
          <li><code>{item.old_title}</code> → merge into <code>{item.new_title}</code></li>
        {/each}
        {#if bulkPreview.merged.length > 8}
          <li>…and {bulkPreview.merged.length - 8} more</li>
        {/if}
      </ul>
    {/if}
    {#if bulkPreview.skipped.length > 0}
      <ul class="bulk-rename-examples skipped">
        {#each bulkPreview.skipped.slice(0, 5) as item, index (`${item.old_title}-${index}`)}
          <li><code>{item.old_title}</code> — {item.reason}</li>
        {/each}
      </ul>
    {/if}
  {/if}

  <div class="browser-controls">
    <div class="control-group" role="group" aria-label="Page browser view">
      <button
        class="mode-btn"
        class:active={viewMode === "tree"}
        aria-pressed={viewMode === "tree"}
        disabled={pageTreeAvailable === false}
        title={pageTreeAvailable === false ? "Page trees are unavailable in this build" : undefined}
        onclick={() => { viewMode = "tree"; }}
      >
        Tree
      </button>
      <button
        class="mode-btn"
        class:active={viewMode === "list"}
        aria-pressed={viewMode === "list"}
        onclick={() => { viewMode = "list"; }}
      >
        List
      </button>
    </div>

    {#if viewMode === "tree"}
      <div class="control-group" role="group" aria-label="Tree source">
        <button
          class="mode-btn"
          class:active={treeSource === "namespace"}
          aria-pressed={treeSource === "namespace"}
          onclick={() => { treeSource = "namespace"; }}
        >
          Namespace
        </button>
        <button
          class="mode-btn"
          class:active={treeSource === "tags"}
          aria-pressed={treeSource === "tags"}
          onclick={() => { treeSource = "tags"; }}
        >
          Tags
        </button>
      </div>
    {/if}

    <div class="control-group" role="group" aria-label="Page kind">
      <button
        class="mode-btn"
        class:active={kindFilter === "all"}
        aria-pressed={kindFilter === "all"}
        title="Every page, written or not"
        onclick={() => setKindFilter("all")}
      >
        All
      </button>
      <button
        class="mode-btn"
        class:active={kindFilter === "filed"}
        aria-pressed={kindFilter === "filed"}
        title="Only pages with a markdown file on disk"
        onclick={() => setKindFilter("filed")}
      >
        Files
      </button>
      <button
        class="mode-btn"
        class:active={kindFilter === "virtual"}
        aria-pressed={kindFilter === "virtual"}
        title="Only placeholders — pages a [[link]] or #tag named but nothing has written yet"
        onclick={() => setKindFilter("virtual")}
      >
        Placeholders
      </button>
    </div>

    <div class="control-group" role="group" aria-label="Sort order">
      <button
        class="mode-btn"
        class:active={!sortByTitle}
        aria-pressed={!sortByTitle}
        title="Folders first, then newest. A folder counts as its most recently edited page."
        onclick={() => setSort(false)}
      >
        Recent
      </button>
      <button
        class="mode-btn"
        class:active={sortByTitle}
        aria-pressed={sortByTitle}
        title="Folders first, then alphabetical"
        onclick={() => setSort(true)}
      >
        A–Z
      </button>
    </div>
  </div>

  {#if total === 0}
    <div class="empty-state">
      {#if kindFilter === "filed"}
        <p>No pages with a file on disk. Every page here is still a placeholder.</p>
      {:else if kindFilter === "virtual"}
        <p>No placeholders — every page a link or tag names has been written.</p>
      {:else}
        <p>No pages yet. Create one above!</p>
      {/if}
    </div>
  {:else}
    <!-- Tree view only — see `filterQuery`. -->
    <div class="page-filter" hidden={viewMode !== "tree"}>
      <input
        type="search"
        class="page-filter-input"
        data-local-search
        placeholder="Filter pages…"
        aria-label="Filter pages"
        bind:value={filterQuery}
      />
      {#if filterQuery.trim()}
        <span class="page-filter-count" aria-live="polite">
          {filteredCount} match{filteredCount === 1 ? "" : "es"}
        </span>
      {/if}
    </div>
  {/if}

  {#if total === 0}
    <!-- handled above -->
  {:else if viewMode === "tree"}
    <div class="tree-browser" aria-busy={pageTreeLoading}>
      {#if pageTreeLoading && pageTree.length === 0}
        <p class="tree-message shimmer">Loading {treeSource === "namespace" ? "namespace" : "tag"} tree…</p>
      {:else if pageTreeError}
        <div class="tree-error" role="alert">
          <p>Could not load the {treeSource === "namespace" ? "namespace" : "tag"} tree.</p>
          <button type="button" onclick={() => loadPageTree(treeSource)}>Try again</button>
        </div>
      {:else}
        <PageTree
          nodes={visibleTree}
          columns
          revealToken={filterQuery.trim()}
          {onNavigate}
          onPageContextMenu={handleTreeNodeMenu}
          hasPageMenu={(node) => treeSource === "namespace" || node.page_id !== null}
          storageKey={`${graphScopedKey(ALL_PAGES_TREE_STORAGE_KEY, graphPath)}.${treeSource}`}
          ariaLabel={treeSource === "namespace" ? "Pages by namespace" : "Pages by tag"}
          emptyText={treeSource === "namespace"
            ? "No page namespaces yet. Use / in a page title to build one."
            : "No tagged pages yet."}
        />
      {/if}
    </div>
  {:else}
    <div class="pages-spacer" bind:this={spacerEl} style="height: {total * ROW_H}px;">
      {#each visible as i (i)}
        {@const page = rows.get(i)}
        <div class="page-row" style="top: {i * ROW_H}px;">
          {#if page}
            <button
              class="page-link"
              onclick={() => onNavigate(page.title)}
              oncontextmenu={(event) => {
                event.preventDefault();
                handleListPageMenu(event, page);
              }}
            >
              {page.title}
              {#if page.is_journal}
                <span class="badge">Journal</span>
              {/if}
            </button>
            <span class="page-date">{fmtDate(page.updated_at)}</span>
            <button
              type="button"
              class="btn-page-actions"
              onclick={(event) => handleListPageMenu(event, page)}
              title="Page actions"
              aria-label={`Actions for ${page.title}`}
            >⋯</button>
          {:else}
            <span class="page-link placeholder">…</span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if actionMenu}
    <div
      class="page-action-menu"
      role="menu"
      aria-label={`Actions for ${actionMenu.title}`}
      style={`left: ${actionMenu.x}px; top: ${actionMenu.y}px;`}
      bind:this={actionMenuEl}
    >
      <p class="page-action-title">{actionMenu.bookTitle ?? actionMenu.title}</p>
      {#if !actionMenu.bookTitle}
        <button type="button" role="menuitem" class="page-action-item" onclick={() => void handleRenameActionTarget()}>
          Rename{actionMenu.folder ? " folder" : ""}
        </button>
      {/if}
      {#if actionMenu.pageId || actionMenu.bookTitle || actionMenu.folder}
        <button type="button" role="menuitem" class="page-action-item" onclick={handleOpenInFileBrowser}>
          Open in file browser
        </button>
      {/if}
      <div class="page-action-separator" role="separator"></div>
      <button type="button" role="menuitem" class="page-action-item danger" onclick={() => void handleDeleteActionTarget()}>
        {#if actionMenu.bookTitle}
          Delete whole book folder
        {:else if actionMenu.folder}
          Delete folder
        {:else}
          Delete page
        {/if}
      </button>
    </div>
  {/if}

  {#if confirmDialog}
    <div class="page-dialog-backdrop" role="presentation" onclick={() => answerConfirm(false)}>
      <div
        class="page-dialog"
        role="alertdialog"
        tabindex="-1"
        aria-modal="true"
        aria-label={confirmDialog.confirmLabel}
        onclick={(event) => event.stopPropagation()}
        onkeydown={(event) => {
          if (event.key === "Escape") answerConfirm(false);
        }}
      >
        <p class="page-dialog-message">{confirmDialog.message}</p>
        <div class="page-dialog-actions">
          <button type="button" class="page-dialog-btn" onclick={() => answerConfirm(false)}>Cancel</button>
          <button type="button" class="page-dialog-btn danger" onclick={() => answerConfirm(true)}>
            {confirmDialog.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  {/if}

  {#if renameDialog}
    <div class="page-dialog-backdrop" role="presentation" onclick={() => { if (!renameBusy) renameDialog = null; }}>
      <div
        class="page-dialog"
        role="dialog"
        tabindex="-1"
        aria-modal="true"
        aria-label="Rename"
        onclick={(event) => event.stopPropagation()}
        onkeydown={(event) => {
          if (event.key === "Escape" && !renameBusy) renameDialog = null;
          if (event.key === "Enter") void commitRenameDialog();
        }}
      >
        <p class="page-dialog-message">
          {renameDialog.folder ? `Rename folder '${renameDialog.title}'` : `Rename '${renameDialog.title}'`}
        </p>
        <input
          class="new-page-input"
          bind:this={renameInputEl}
          bind:value={renameDialog.draft}
          disabled={renameBusy}
        />
        {#if renameDialog.folder}
          <p class="page-dialog-hint">Pages under this folder are renamed too.</p>
        {/if}
        {#if renameError}
          <p class="bulk-rename-status error">{renameError}</p>
        {/if}
        <div class="page-dialog-actions">
          <button type="button" class="page-dialog-btn" onclick={() => { if (!renameBusy) renameDialog = null; }} disabled={renameBusy}>
            Cancel
          </button>
          <button type="button" class="page-dialog-btn primary" onclick={() => void commitRenameDialog()} disabled={renameBusy}>
            Rename
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  /* Wide, because this is an index to scan rather than prose to read: at 920px
     a few hundred pages became one long column with most of the window empty.
     Still capped so the tree does not stretch into unreadably long rows on an
     ultrawide display. */
  .all-pages {
    max-width: 1680px;
    margin: 0 auto;
    padding: 40px 24px;
  }

  /* The controls are single fields, so they follow the old measure instead of
     growing with the page. */
  .controls,
  .page-filter {
    max-width: 920px;
  }

  .header {
    display: flex;
    align-items: baseline;
    gap: 12px;
    margin-bottom: 24px;
  }

  .page-title {
    font-size: 32px;
    font-weight: 700;
    margin: 0;
    color: var(--text-primary);
  }

  .count {
    font-size: 13px;
    color: var(--text-muted);
  }

  .controls {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 20px;
    gap: 12px;
    flex-wrap: wrap;
  }

  .new-page {
    display: flex;
    gap: 8px;
    flex: 1;
  }

  .new-page-input {
    flex: 1;
    padding: 8px 12px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: 14px;
    outline: none;
  }

  /* A border-colour change alone is easy to miss, and `outline: none` above
     removed the only other cue. Keyboard focus gets a real ring. */
  .new-page-input:focus-visible {
    border-color: var(--accent);
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .new-page-input:focus {
    border-color: var(--accent);
  }

  .btn-create {
    padding: 8px 16px;
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
  }

  .btn-create:hover {
    background: var(--btn-primary-hover);
  }

  .bulk-rename {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 12px;
    flex-wrap: wrap;
  }

  .bulk-rename-label {
    font-size: 13px;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .bulk-rename .new-page-input {
    min-width: 140px;
    flex: 1;
  }

  .bulk-rename-arrow {
    color: var(--text-muted);
  }

  .bulk-rename-status {
    margin: -4px 0 12px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .bulk-rename-status.error {
    color: var(--danger, #c44);
  }

  .bulk-rename-examples {
    margin: -4px 0 12px;
    padding-left: 18px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .bulk-rename-examples code {
    color: var(--text-primary);
  }

  .browser-controls {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 18px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--border);
  }

  .control-group {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 2px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--bg-secondary);
  }

  .btn-import-media {
    padding: 8px 16px;
    background: var(--btn-bg, transparent);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-secondary);
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }

  .btn-import-media:hover {
    background: var(--btn-bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .mode-btn {
    padding: 6px 12px;
    min-height: 32px;
    background: transparent;
    border: none;
    border-radius: 5px;
    color: var(--text-secondary);
    font-size: 12px;
    cursor: pointer;
  }

  .mode-btn.active {
    background: var(--bg-active);
    color: var(--text-primary);
  }

  .mode-btn:hover:not(:disabled) {
    color: var(--text-primary);
  }

  .mode-btn:focus-visible,
  .tree-error button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .mode-btn:disabled {
    color: var(--text-muted);
    cursor: not-allowed;
    opacity: 0.65;
  }

  .page-filter {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0 0 10px;
  }

  .page-filter-input {
    flex: 1;
    padding: 7px 10px;
    border: 1px solid var(--border-color);
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    font: inherit;
    font-size: 13px;
  }

  .page-filter-input:focus-visible {
    outline: 2px solid var(--text-link);
    outline-offset: 1px;
  }

  .page-filter-count {
    flex: none;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .tree-browser {
    min-height: 160px;
  }

  .tree-message,
  .tree-error {
    margin: 0;
    padding: 26px 10px;
    color: var(--text-secondary);
    font-size: 13px;
  }

  .tree-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .tree-error p {
    margin: 0;
  }

  .tree-error button {
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--btn-bg);
    color: var(--text-secondary);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }

  .tree-error button:hover {
    background: var(--btn-bg-hover);
    color: var(--text-primary);
  }

  .pages-spacer {
    position: relative;
    width: 100%;
  }

  .page-row {
    position: absolute;
    left: 0;
    right: 0;
    height: 44px;
    box-sizing: border-box;
    display: flex;
    align-items: center;
    padding: 8px 12px;
    border-radius: 6px;
    gap: 12px;
  }

  .page-row:hover {
    background: var(--bg-hover);
  }

  .page-link {
    flex: 1;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: 15px;
    cursor: pointer;
    text-align: left;
    display: flex;
    align-items: center;
    gap: 8px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .page-link:hover {
    color: var(--accent);
  }

  .page-link.placeholder {
    color: var(--text-muted);
    cursor: default;
  }

  .badge {
    font-size: 10px;
    padding: 2px 6px;
    background: var(--bg-secondary);
    border-radius: 4px;
    color: var(--text-muted);
  }

  .page-date {
    font-size: 12px;
    color: var(--text-muted);
    white-space: nowrap;
  }

  .btn-page-actions {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 18px;
    cursor: pointer;
    padding: 2px 6px;
    min-width: 32px;
    min-height: 32px;
    border-radius: 4px;
    opacity: 0.7;
  }

  .page-row:hover .btn-page-actions,
  .page-row:focus-within .btn-page-actions {
    opacity: 1;
  }

  .btn-page-actions:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-page-actions:focus-visible {
    opacity: 1;
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .page-action-menu {
    position: fixed;
    z-index: 100;
    width: 228px;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-overlay);
    box-shadow: 0 10px 28px color-mix(in srgb, var(--bg-primary) 70%, transparent);
  }

  .page-action-title {
    margin: 0 0 5px;
    padding: 5px 7px 7px;
    border-bottom: 1px solid var(--border);
    color: var(--text-muted);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .page-action-item {
    width: 100%;
    padding: 8px 9px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-primary);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .page-action-item:hover {
    background: var(--bg-hover);
  }

  .page-action-item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .page-action-item.danger {
    color: var(--danger);
  }

  .page-action-item.danger:hover {
    background: var(--danger-bg);
  }

  .page-action-separator {
    height: 1px;
    margin: 5px 2px;
    background: var(--border);
  }

  .page-dialog-backdrop {
    position: fixed;
    inset: 0;
    z-index: 120;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--bg-primary) 55%, transparent);
  }

  .page-dialog {
    width: min(440px, calc(100vw - 32px));
    padding: 16px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface-overlay, var(--bg-secondary));
    box-shadow: 0 16px 40px color-mix(in srgb, var(--bg-primary) 70%, transparent);
  }

  .page-dialog-message {
    margin: 0 0 12px;
    color: var(--text-primary);
    font-size: 14px;
    line-height: 1.45;
  }

  .page-dialog-hint {
    margin: 8px 0 0;
    color: var(--text-muted);
    font-size: 12px;
  }

  .page-dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }

  .page-dialog-btn {
    padding: 7px 12px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--btn-bg, transparent);
    color: var(--text-primary);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  .page-dialog-btn:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .page-dialog-btn.primary {
    background: var(--btn-primary-bg);
    border-color: transparent;
    color: var(--btn-primary-fg);
  }

  .page-dialog-btn.danger {
    background: var(--danger, #c44);
    border-color: transparent;
    color: #fff;
  }

  .page-dialog-btn:disabled {
    opacity: 0.65;
    cursor: not-allowed;
  }

  .empty-state {
    text-align: center;
    padding: 60px 20px;
    color: var(--text-muted);
  }

  @media (max-width: 640px) {
    .all-pages {
      padding: 24px 14px 88px;
    }

    .controls {
      align-items: stretch;
    }

    .new-page {
      min-width: 100%;
    }

    .btn-import-media {
      flex: 1;
    }

    .browser-controls {
      align-items: stretch;
    }

    .control-group {
      flex: 1;
    }

    .mode-btn {
      flex: 1;
      padding-inline: 8px;
    }
  }
</style>
