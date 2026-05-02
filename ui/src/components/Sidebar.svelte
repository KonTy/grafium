<script lang="ts">
  import { tick } from "svelte";
  import GraphMenu from "./GraphMenu.svelte";
  import { listFavorites, listRecentPages, listPages, listJournalPages, searchFts, getPage, addFavorite, removeFavorite } from "../lib/api";
  import type { Page, Block } from "../lib/api";

  interface Props {
    currentPage?: Page | null;
    onNavigate: (pageTitle: string) => void;
    onGraphChanged?: () => void;
  }

  let { currentPage = null, onNavigate, onGraphChanged = () => {} }: Props = $props();

  let favorites: Page[] = $state([]);
  let recentPages: Page[] = $state([]);
  let searchQuery = $state("");
  let searchResults: Array<
    { kind: "page"; page: Page }
    | { kind: "block"; block: Block }
  > = $state([]);
  let showSearch = $state(false);
  let searchInputEl: HTMLInputElement | null = $state(null);
  let allSearchPages: Page[] = $state([]);
  let pagesLoaded = $state(false);

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

  function normalizeForFuzzy(value: string): string {
    return value.toLowerCase().replace(/[^a-z0-9]/g, "");
  }

  function isSubsequence(needle: string, haystack: string): boolean {
    let i = 0;
    let j = 0;
    while (i < needle.length && j < haystack.length) {
      if (needle[i] === haystack[j]) i += 1;
      j += 1;
    }
    return i === needle.length;
  }

  function toIsoDate(year: number, month: number, day: number): string | null {
    const dt = new Date(year, month - 1, day);
    if (
      dt.getFullYear() !== year ||
      dt.getMonth() !== month - 1 ||
      dt.getDate() !== day
    ) {
      return null;
    }
    return `${year}-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
  }

  function normalizeDateInput(raw: string): string | null {
    const value = raw.trim();
    if (!value) return null;

    let match = value.match(/^(\d{4})-(\d{1,2})-(\d{1,2})$/);
    if (match) {
      return toIsoDate(Number(match[1]), Number(match[2]), Number(match[3]));
    }

    match = value.match(/^(\d{1,2})[\/-](\d{1,2})[\/-](\d{2}|\d{4})$/);
    if (match) {
      const month = Number(match[1]);
      const day = Number(match[2]);
      const yrRaw = Number(match[3]);
      const year = match[3].length === 2 ? 2000 + yrRaw : yrRaw;
      return toIsoDate(year, month, day);
    }

    return null;
  }

  function scorePageTitle(title: string, query: string, isoDateQuery: string | null): number | null {
    const q = query.toLowerCase();
    const t = title.toLowerCase();
    const qn = normalizeForFuzzy(query);
    const tn = normalizeForFuzzy(title);

    if (isoDateQuery && title === isoDateQuery) return 1200;

    const exact = t.indexOf(q);
    if (exact >= 0) return 1000 - exact;

    if (qn.length > 0) {
      const fuzzyContains = tn.indexOf(qn);
      if (fuzzyContains >= 0) return 800 - fuzzyContains;
      if (isSubsequence(qn, tn)) return 500;
    }

    return null;
  }

  async function ensureSearchPagesLoaded() {
    if (pagesLoaded) return;

    const [journalPages, regularPages] = await Promise.all([
      listJournalPages(5000, 0),
      listPages(5000, 0),
    ]);

    const deduped = new Map<string, Page>();
    for (const p of [...journalPages, ...regularPages]) {
      const key = p.title.toLowerCase();
      if (!deduped.has(key)) deduped.set(key, p);
    }

    allSearchPages = Array.from(deduped.values());
    pagesLoaded = true;
  }

  async function handleSearch() {
    if (searchQuery.trim().length < 1) {
      searchResults = [];
      return;
    }

    await ensureSearchPagesLoaded();

    const query = searchQuery.trim();
    const isoDateQuery = normalizeDateInput(query);

    const pageMatches = allSearchPages
      .map((page) => ({ page, score: scorePageTitle(page.title, query, isoDateQuery) }))
      .filter((x): x is { page: Page; score: number } => x.score !== null)
      .sort((a, b) => b.score - a.score || a.page.title.localeCompare(b.page.title))
      .slice(0, 10)
      .map((x) => ({ kind: "page" as const, page: x.page }));

    const blockMatches = query.length >= 2
      ? (await searchFts(query, 20)).slice(0, 12).map((block) => ({ kind: "block" as const, block }))
      : [];

    searchResults = [...pageMatches, ...blockMatches];
  }

  function handleSearchKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      showSearch = false;
      searchQuery = "";
      searchResults = [];
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
      searchQuery = "";
      searchResults = [];
      return;
    }
    void openSearch();
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
    searchQuery = "";
    searchResults = [];
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

  function navigateToPageResult(page: Page) {
    showSearch = false;
    searchQuery = "";
    searchResults = [];
    onNavigate(page.title);
  }
</script>

<aside class="sidebar">
  <div class="sidebar-header">
    <GraphMenu onGraphChanged={() => { loadSidebar(); pagesLoaded = false; allSearchPages = []; onGraphChanged(); }} />
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
        oninput={handleSearch}
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
    <button class="nav-item" onclick={() => onNavigate("__all_pages__")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
        <polyline points="14 2 14 8 20 8"></polyline>
      </svg>
      <span>All Pages</span>
    </button>
    <button class="nav-item" onclick={() => onNavigate("__flashcards__")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <rect x="2" y="4" width="20" height="16" rx="2"></rect>
        <path d="M12 8v8"></path>
        <path d="M8 12h8"></path>
      </svg>
      <span>Flashcards</span>
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

  <div class="sidebar-spacer"></div>

  <div class="sidebar-footer">
    <button class="create-btn" onclick={() => onNavigate("__new_page__")}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <line x1="12" y1="5" x2="12" y2="19"></line>
        <line x1="5" y1="12" x2="19" y2="12"></line>
      </svg>
      <span>Create</span>
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

  .sidebar-spacer {
    flex: 1;
  }

  .sidebar-footer {
    padding-top: 12px;
    border-top: 1px solid var(--border);
    margin-top: 12px;
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
