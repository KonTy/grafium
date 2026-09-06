<script lang="ts">
  import { SvelteMap } from "svelte/reactivity";
  import PageTree from "./PageTree.svelte";
  import { countPages, listPagesWindow, createPage, deletePage, getGraphInfo } from "../lib/api";
  import {
    getPageTree,
    toPageTreeView,
    withMissingCommandFallback,
    type PageTreeSource,
  } from "../lib/pageTree";
  import {
    ALL_PAGES_TREE_STORAGE_KEY,
    graphScopedKey,
    filterTreeByQuery,
    countTreePages,
    type PageTreeViewNode,
  } from "../lib/pageTreeState";
  import type { Page } from "../lib/api";

  interface Props {
    onNavigate: (title: string) => void;
  }

  let { onNavigate }: Props = $props();

  // Fixed-height virtual list tuned for millions of rows: only the rows in (or
  // near) the viewport are ever in the DOM, and each window is fetched from the
  // DB on demand via offset paging off a partial index (~20ms even at offset
  // 900k). Nothing is capped — the full data set is browsable.
  const ROW_H = 44; // px per row, must match .page-row height
  const CHUNK = 200; // rows fetched per DB request
  const OVERSCAN = 8; // extra rows rendered above/below the viewport

  let total = $state(0);
  let sortByTitle = $state(false); // false = Recent (updated_at), true = A-Z (title)
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
  let visibleTree = $derived(filterTreeByQuery(pageTree, filterQuery));
  let filteredCount = $derived(countTreePages(visibleTree));

  // Loaded rows keyed by absolute index; SvelteMap is reactive so the template
  // updates as windows stream in.
  let rows = new SvelteMap<number, Page>();
  const requested = new Set<number>(); // chunk indices already fetched
  let reloadToken = $state(0); // bump to force a re-fetch of the visible window

  let spacerEl: HTMLDivElement | null = $state(null);
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
  $effect(() => {
    void getGraphInfo()
      .then((info) => { graphPath = info.path; })
      .catch(() => { graphPath = null; });
  });

  $effect(() => {
    const source = treeSource;
    if (viewMode !== "tree") return;
    void loadPageTree(source);
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
    ensureVisibleLoaded();
  });

  function ensureVisibleLoaded() {
    if (endIndex <= startIndex) return;
    const byTitle = sortByTitle;
    const firstChunk = Math.floor(startIndex / CHUNK);
    const lastChunk = Math.floor((endIndex - 1) / CHUNK);
    for (let ch = firstChunk; ch <= lastChunk; ch++) {
      if (requested.has(ch)) continue;
      requested.add(ch);
      void fetchChunk(ch, byTitle);
    }
  }

  async function fetchChunk(chunk: number, byTitle: boolean) {
    const offset = chunk * CHUNK;
    try {
      const pages = await listPagesWindow(CHUNK, offset, byTitle);
      // Ignore late responses from a superseded sort.
      if (byTitle !== sortByTitle) return;
      for (let k = 0; k < pages.length; k++) rows.set(offset + k, pages[k]);
    } catch (e) {
      requested.delete(chunk); // allow a retry
      console.error("Failed to load page window:", e);
    }
  }

  async function refreshCount() {
    try {
      total = await countPages();
    } catch (e) {
      console.error("Failed to count pages:", e);
    }
  }

  async function loadPageTree(source: PageTreeSource) {
    const request = ++pageTreeRequest;
    pageTreeLoading = true;
    pageTreeError = "";
    pageTree = [];
    try {
      const result = await withMissingCommandFallback(
        () => getPageTree(source),
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

  function setSort(byTitle: boolean) {
    if (byTitle === sortByTitle) return;
    sortByTitle = byTitle;
    resetWindows();
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

  async function handleDeletePage(page: Page) {
    await deletePage(page.id);
    resetWindows();
    await refreshCount();
    if (pageTreeAvailable !== false) void loadPageTree(treeSource);
    window.dispatchEvent(new CustomEvent("page-tree-refresh"));
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
    {:else}
      <div class="control-group" role="group" aria-label="Page list sort">
        <button class="mode-btn" class:active={!sortByTitle} aria-pressed={!sortByTitle} onclick={() => setSort(false)}>Recent</button>
        <button class="mode-btn" class:active={sortByTitle} aria-pressed={sortByTitle} onclick={() => setSort(true)}>A–Z</button>
      </div>
    {/if}
  </div>

  {#if total === 0}
    <div class="empty-state">
      <p>No pages yet. Create one above!</p>
    </div>
  {:else}
    <!-- Tree view only — see `filterQuery`. -->
    <div class="page-filter" hidden={viewMode !== "tree"}>
      <input
        type="search"
        class="page-filter-input"
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
        <p class="tree-message">Loading {treeSource === "namespace" ? "namespace" : "tag"} tree…</p>
      {:else if pageTreeError}
        <div class="tree-error" role="alert">
          <p>Could not load the {treeSource === "namespace" ? "namespace" : "tag"} tree.</p>
          <button type="button" onclick={() => loadPageTree(treeSource)}>Try again</button>
        </div>
      {:else}
        <PageTree
          nodes={visibleTree}
          columns
          {onNavigate}
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
            <button class="page-link" onclick={() => onNavigate(page.title)}>
              {page.title}
              {#if page.is_journal}
                <span class="badge">Journal</span>
              {/if}
            </button>
            <span class="page-date">{fmtDate(page.updated_at)}</span>
            <button class="btn-delete" onclick={() => handleDeletePage(page)} title="Delete">×</button>
          {:else}
            <span class="page-link placeholder">…</span>
          {/if}
        </div>
      {/each}
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

  .btn-delete {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 18px;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 4px;
    opacity: 0;
  }

  .page-row:hover .btn-delete {
    opacity: 1;
  }

  .btn-delete:hover {
    background: var(--danger-bg);
    color: var(--danger);
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
