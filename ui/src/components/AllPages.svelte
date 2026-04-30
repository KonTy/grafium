<script lang="ts">
  import { listPages, createPage, deletePage } from "../lib/api";
  import type { Page } from "../lib/api";

  interface Props {
    onNavigate: (title: string) => void;
  }

  let { onNavigate }: Props = $props();

  let pages: Page[] = $state([]);
  let newPageTitle = $state("");
  let sortBy: "title" | "updated" = $state("updated");

  $effect(() => {
    loadPages();
  });

  async function loadPages() {
    pages = await listPages(500, 0);
  }

  let sortedPages = $derived(
    [...pages].sort((a, b) => {
      if (sortBy === "title") return a.title.localeCompare(b.title);
      return new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime();
    })
  );

  async function handleCreatePage() {
    if (!newPageTitle.trim()) return;
    await createPage(newPageTitle.trim());
    newPageTitle = "";
    await loadPages();
  }

  async function handleDeletePage(id: string) {
    await deletePage(id);
    await loadPages();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") handleCreatePage();
  }
</script>

<div class="all-pages">
  <h1 class="page-title">All Pages</h1>

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
    <div class="sort-controls">
      <button class="sort-btn" class:active={sortBy === "updated"} onclick={() => (sortBy = "updated")}>Recent</button>
      <button class="sort-btn" class:active={sortBy === "title"} onclick={() => (sortBy = "title")}>A-Z</button>
    </div>
  </div>

  <div class="pages-list">
    {#each sortedPages as page (page.id)}
      <div class="page-row">
        <button class="page-link" onclick={() => onNavigate(page.title)}>
          {page.title}
          {#if page.is_journal}
            <span class="badge">Journal</span>
          {/if}
        </button>
        <span class="page-date">{new Date(page.updated_at).toLocaleDateString()}</span>
        <button class="btn-delete" onclick={() => handleDeletePage(page.id)} title="Delete">×</button>
      </div>
    {/each}
  </div>

  {#if pages.length === 0}
    <div class="empty-state">
      <p>No pages yet. Create one above!</p>
    </div>
  {/if}
</div>

<style>
  .all-pages {
    max-width: 800px;
    margin: 0 auto;
    padding: 40px 24px;
  }

  .page-title {
    font-size: 32px;
    font-weight: 700;
    margin-bottom: 24px;
    color: var(--text-primary);
  }

  .controls {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 20px;
    gap: 12px;
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
    background: var(--accent);
    color: white;
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
  }

  .btn-create:hover {
    opacity: 0.9;
  }

  .sort-controls {
    display: flex;
    gap: 4px;
  }

  .sort-btn {
    padding: 6px 12px;
    background: none;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-secondary);
    font-size: 12px;
    cursor: pointer;
  }

  .sort-btn.active {
    background: var(--bg-active);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .pages-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .page-row {
    display: flex;
    align-items: center;
    padding: 10px 12px;
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
  }

  .page-link:hover {
    color: var(--accent);
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
</style>
