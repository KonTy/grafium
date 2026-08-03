<script lang="ts">
  import { tick } from "svelte";
  import GraphMenu from "./GraphMenu.svelte";
  import { listFavorites, listRecentPages, getPage, addFavorite, removeFavorite } from "../lib/api";
  import { createSidebarSearchController, runSidebarSearch } from "../lib/sidebarSearch";
  import type { Page, PageSummary, Block } from "../lib/api";
  import type { SidebarSearchResult } from "../lib/sidebarSearch";

  interface Props {
    currentPage?: Page | null;
    onNavigate: (pageTitle: string) => void;
    onGraphChanged?: () => void;
    sidebarWidth?: number;
  }

  let { currentPage = null, onNavigate, onGraphChanged = () => {}, sidebarWidth = 260 }: Props = $props();

  const COMPACT_SIDEBAR_WIDTH = 220;
  let compactSidebar = $derived(sidebarWidth < COMPACT_SIDEBAR_WIDTH);

  let favorites: Page[] = $state([]);
  let recentPages: Page[] = $state([]);
  let searchQuery = $state("");
  let searchResults: SidebarSearchResult[] = $state([]);
  let showSearch = $state(false);
  let searchInputEl: HTMLInputElement | null = $state(null);

  // Context menu state
  interface ContextMenu {
    x: number;
    y: number;
    page: Page;
    isFav: boolean;
  }
  let contextMenu: ContextMenu | null = $state(null);

  $effect(() => {
    loadSidebar();
  });

  // Refresh recent pages whenever currentPage changes
  $effect(() => {
    // eslint-disable-next-line @typescript-eslint/no-unused-expressions
    currentPage;
    listRecentPages(10).then((p) => { recentPages = p; }).catch(() => {});
  });

  // Close context menu on any click outside
  $effect(() => {
    function closeMenu() { contextMenu = null; }
    window.addEventListener("click", closeMenu);
    window.addEventListener("contextmenu", closeMenu);
    return () => {
      window.removeEventListener("click", closeMenu);
      window.removeEventListener("contextmenu", closeMenu);
    };
  });

  async function loadSidebar() {
    try {
      favorites = await listFavorites();
    } catch { favorites = []; }
    try {
      recentPages = await listRecentPages(10);
    } catch { recentPages = []; }
  }

  function favSet(): Set<string> {
    return new Set(favorites.map((f) => f.id));
  }

  function handlePageRightClick(e: MouseEvent, page: Page) {
    e.preventDefault();
    e.stopPropagation();
    contextMenu = { x: e.clientX, y: e.clientY, page, isFav: favSet().has(page.id) };
  }

  async function handleToggleFavorite() {
    if (!contextMenu) return;
    const { page, isFav } = contextMenu;
    contextMenu = null;
    if (isFav) {
      await removeFavorite(page.id).catch(() => {});
    } else {
      await addFavorite(page.id).catch(() => {});
    }
    favorites = await listFavorites().catch(() => []);
  }

  const searchController = createSidebarSearchController<SidebarSearchResult[]>({
    debounceMs: 120,
    run: runSidebarSearch,
    apply: (_query, results) => {
      searchResults = results;
    },
    clear: () => {
      searchResults = [];
    },
  });

  $effect(() => {
    return () => {
      searchController.cancel();
    };
  });

  function handleSearchInput() {
    searchController.submit(searchQuery);
  }

  function clearSearch(resetQuery = false) {
    searchController.cancel();
    if (resetQuery) {
      searchQuery = "";
    }
    searchResults = [];
  }

  function handleSearchKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      showSearch = false;
      clearSearch(true);
    }
  }

  async function openSearch() {
    showSearch = true;
    await tick();
    searchInputEl?.focus();
    searchInputEl?.select();
  }

  function toggleSearch() {
    if (showSearch) {
      showSearch = false;
      clearSearch(true);
      return;
    }
    void openSearch();
  }

  function resetSearchState() {
    clearSearch(true);
  }

  function handleSidebarGraphChanged() {
    void loadSidebar();
    resetSearchState();
    onGraphChanged();
  }

  $effect(() => {
    const handleToggleSearch = () => {
      toggleSearch();
    };
    window.addEventListener("toggle-search", handleToggleSearch);
    return () => {
      window.removeEventListener("toggle-search", handleToggleSearch);
    };
  });

  // Cache page titles to avoid repeated lookups
  const pageTitleCache = new Map<string, string>();

  async function navigateToBlock(result: Block) {
    showSearch = false;
    clearSearch(true);
    try {
      let title = pageTitleCache.get(result.page_id);
      if (!title) {
        const page = await getPage({ id: result.page_id });
        title = page.title;
        pageTitleCache.set(result.page_id, title);
      }
      window.dispatchEvent(new CustomEvent("navigate-page", {
        detail: {
          pageName: title,
          targetBlockId: result.id,
        },
      }));
    } catch (e) {
      console.error("Search navigation failed:", e);
    }
  }

  function navigateToJournal() {
    onNavigate("__journal__");
  }

  function navigateToPageResult(page: PageSummary) {
    showSearch = false;
    clearSearch(true);
    onNavigate(page.title);
  }
</script>

<aside class="sidebar">
  <div class="sidebar-header">
    <GraphMenu onGraphChanged={handleSidebarGraphChanged} />
    <button class="search-toggle" onclick={toggleSearch} title="Search (Ctrl+K)">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <circle cx="11" cy="11" r="8"></circle>
        <path d="m21 21-4.35-4.35"></path>
      </svg>
    </button>
  </div>

  {#if showSearch}
    <div class="search-container">
      <input
        type="text"
        class="search-input"
        placeholder="Search pages & blocks..."
        bind:this={searchInputEl}
        bind:value={searchQuery}
        oninput={handleSearchInput}
        onkeydown={handleSearchKeydown}
      />
      {#if searchResults.length > 0}
        <div class="search-results">
          {#each searchResults as result}
            {#if result.kind === "page"}
              <button class="search-result-item" onclick={() => navigateToPageResult(result.page)}>
                <span class="result-kind">Page</span>
                <span class="result-content">{result.page.title}</span>
              </button>
            {:else}
              <button class="search-result-item" onclick={() => navigateToBlock(result.block)}>
                <span class="result-kind">Block</span>
                <span class="result-content">{result.block.content.replace(/^[-*>\s#]+/, "").slice(0, 100) || "(empty block)"}</span>
              </button>
            {/if}
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <nav class="nav-items">
    <button class="nav-item" onclick={navigateToJournal}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <rect x="3" y="4" width="18" height="18" rx="2" ry="2"></rect>
        <line x1="16" y1="2" x2="16" y2="6"></line>
        <line x1="8" y1="2" x2="8" y2="6"></line>
        <line x1="3" y1="10" x2="21" y2="10"></line>
      </svg>
      <span>Journal</span>
    </button>
    <button class="nav-item" onclick={() => onNavigate("__statistics__")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M18 20V10"></path>
        <path d="M12 20V4"></path>
        <path d="M6 20v-6"></path>
      </svg>
      <span>Statistics</span>
    </button>
    <button class="nav-item" onclick={() => onNavigate("__all_pages__")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
        <polyline points="14 2 14 8 20 8"></polyline>
      </svg>
      <span>All Pages</span>
    </button>
    <button class="nav-item" onclick={() => onNavigate("__graph__")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <circle cx="5" cy="6" r="2"></circle>
        <circle cx="19" cy="6" r="2"></circle>
        <circle cx="12" cy="18" r="2"></circle>
        <line x1="6.7" y1="7" x2="10.5" y2="16.3"></line>
        <line x1="17.3" y1="7" x2="13.5" y2="16.3"></line>
        <line x1="7" y1="6" x2="17" y2="6"></line>
      </svg>
      <span>Graph View</span>
    </button>
    <button class="nav-item" onclick={() => onNavigate("__flashcards__")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <rect x="2" y="4" width="20" height="16" rx="2"></rect>
        <path d="M12 8v8"></path>
        <path d="M8 12h8"></path>
      </svg>
      <span>Flashcards</span>
    </button>
    <button class="nav-item" onclick={() => onNavigate("__chat__")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
      </svg>
      <span>Chatbot</span>
    </button>
    <button class="nav-item" onclick={() => onNavigate("__settings__")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <circle cx="12" cy="12" r="3"></circle>
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
      </svg>
      <span>Settings</span>
    </button>
  </nav>

  {#if favorites.length > 0}
    <div class="sidebar-section">
      <h3 class="section-title">Favorites</h3>
      {#each favorites as fav}
        <button
          class="nav-item page-item"
          class:active={currentPage?.id === fav.id}
          onclick={() => onNavigate(fav.title)}
          oncontextmenu={(e) => handlePageRightClick(e, fav)}
        >
          <svg class="fav-icon" width="12" height="12" viewBox="0 0 24 24" fill="currentColor" stroke="none">
            <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"/>
          </svg>
          {fav.title}
        </button>
      {/each}
    </div>
  {/if}

  {#if recentPages.length > 0}
    <div class="sidebar-section">
      <h3 class="section-title">Recent</h3>
      {#each recentPages as recent}
        <button
          class="nav-item page-item"
          class:active={currentPage?.id === recent.id}
          onclick={() => onNavigate(recent.title)}
          oncontextmenu={(e) => handlePageRightClick(e, recent)}
        >
          {recent.title}
        </button>
      {/each}
    </div>
  {/if}

  <div class="sidebar-footer" class:compact={compactSidebar}>
    <button class="create-btn" class:compact={compactSidebar} onclick={() => onNavigate("__new_page__") } title="Create new page">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <line x1="12" y1="5" x2="12" y2="19"></line>
        <line x1="5" y1="12" x2="19" y2="12"></line>
      </svg>
      {#if !compactSidebar}
        <span>Create</span>
      {/if}
    </button>
    <button class="create-btn" class:compact={compactSidebar} onclick={() => onNavigate("__import_media__") } title="Import from video/audio (URL or file)">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <polygon points="23 7 16 12 23 17 23 7"></polygon>
        <rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect>
      </svg>
      {#if !compactSidebar}
        <span>Import Media</span>
      {/if}
    </button>
  </div>

  {#if contextMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div
      class="context-menu"
      style="top:{contextMenu.y}px;left:{contextMenu.x}px;"
      onclick={(e) => e.stopPropagation()}
    >
      <button class="context-menu-item" onclick={handleToggleFavorite}>
        {#if contextMenu.isFav}
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"/>
          </svg>
          Remove from Favorites
        {:else}
          <svg width="13" height="13" viewBox="0 0 24 24" fill="currentColor" stroke="none">
            <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"/>
          </svg>
          Add to Favorites
        {/if}
      </button>
    </div>
  {/if}
</aside>

<style>
  .sidebar {
    width: 260px;
    height: 100%;
    background: var(--bg-sidebar);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    padding: 16px 12px;
    flex-shrink: 0;
  }

  .sidebar-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .search-toggle {
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 6px;
    border-radius: 4px;
  }

  .search-toggle:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .search-container {
    margin-bottom: 12px;
  }

  .search-input {
    width: 100%;
    padding: 8px 12px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: 13px;
    outline: none;
  }

  .search-input:focus {
    border-color: var(--accent);
  }

  .search-results {
    margin-top: 8px;
    max-height: 300px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .search-result-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    padding: 6px 8px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-secondary);
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .search-result-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .result-kind {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 38px;
    padding: 1px 6px;
    font-size: 10px;
    letter-spacing: 0.2px;
    text-transform: uppercase;
    color: var(--text-muted);
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg-input);
    flex-shrink: 0;
  }

  .result-content {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .nav-items {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 16px;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-secondary);
    font-size: 14px;
    cursor: pointer;
    text-align: left;
    width: 100%;
  }

  .nav-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .nav-item.active {
    background: var(--bg-active);
    color: var(--text-primary);
  }

  .sidebar-section {
    margin-bottom: 16px;
  }

  .section-title {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 4px 10px;
    margin-bottom: 4px;
  }

  .page-item {
    font-size: 13px;
    padding: 6px 10px;
  }

  .sidebar-footer {
    margin-top: auto;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .sidebar-footer.compact {
    padding-top: 0;
    border-top: none;
    flex-direction: row;
    justify-content: center;
    gap: 6px;
  }

  .create-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 12px;
    background: var(--btn-bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-secondary);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .create-btn:hover {
    background: var(--btn-bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .create-btn.compact {
    width: 24px;
    height: 24px;
    padding: 0;
    justify-content: center;
    align-self: center;
    background: transparent;
    border: none;
    border-radius: 0;
  }

  .create-btn.compact:hover {
    background: transparent;
    color: var(--text-primary);
    border: none;
  }

  .fav-icon {
    color: var(--accent);
    flex-shrink: 0;
    opacity: 0.8;
  }

  .context-menu {
    position: fixed;
    z-index: 9999;
    background: var(--bg-sidebar);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.25);
    padding: 4px;
    min-width: 170px;
  }

  .context-menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 7px 10px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-secondary);
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }

  .context-menu-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }
</style>
