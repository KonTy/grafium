<script lang="ts">
  import GraphMenu from "./GraphMenu.svelte";
  import { listFavorites, listRecentPages, listPages, searchFts } from "../lib/api";
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
  let searchResults: Block[] = $state([]);
  let showSearch = $state(false);

  $effect(() => {
    loadSidebar();
  });

  async function loadSidebar() {
    try {
      favorites = await listFavorites();
    } catch { favorites = []; }
    try {
      recentPages = await listRecentPages(10);
    } catch { recentPages = []; }
  }

  async function handleSearch() {
    if (searchQuery.trim().length < 2) {
      searchResults = [];
      return;
    }
    searchResults = await searchFts(searchQuery, 20);
  }

  function handleSearchKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      showSearch = false;
      searchQuery = "";
      searchResults = [];
    }
  }

  function navigateToJournal() {
    const today = new Date().toISOString().split("T")[0];
    onNavigate(today);
  }
</script>

<aside class="sidebar">
  <div class="sidebar-header">
    <GraphMenu onGraphChanged={() => { loadSidebar(); onGraphChanged(); }} />
    <button class="search-toggle" onclick={() => (showSearch = !showSearch)} title="Search (Ctrl+K)">
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
        placeholder="Search blocks..."
        bind:value={searchQuery}
        oninput={handleSearch}
        onkeydown={handleSearchKeydown}
      />
      {#if searchResults.length > 0}
        <div class="search-results">
          {#each searchResults as result}
            <button class="search-result-item" onclick={() => { onNavigate(result.page_id); showSearch = false; }}>
              <span class="result-content">{result.content.slice(0, 80)}</span>
            </button>
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
        <button class="nav-item page-item" class:active={currentPage?.id === fav.id} onclick={() => onNavigate(fav.title)}>
          {fav.title}
        </button>
      {/each}
    </div>
  {/if}

  {#if recentPages.length > 0}
    <div class="sidebar-section">
      <h3 class="section-title">Recent</h3>
      {#each recentPages as recent}
        <button class="nav-item page-item" class:active={currentPage?.id === recent.id} onclick={() => onNavigate(recent.title)}>
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
    display: block;
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
</style>
