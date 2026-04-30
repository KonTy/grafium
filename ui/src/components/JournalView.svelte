<script lang="ts">
  import PageContent from "./PageContent.svelte";
  import { listJournalPages, createPage, getPage } from "../lib/api";
  import type { Page } from "../lib/api";

  let journalPages: Page[] = $state([]);
  let loading = $state(true);
  let loadingMore = $state(false);
  let hasMore = $state(true);

  $effect(() => {
    loadJournals();
  });

  async function loadJournals() {
    loading = true;
    try {
      // Ensure today's journal exists
      const today = new Date().toISOString().split("T")[0];
      try {
        await getPage({ title: today });
      } catch {
        await createPage(today, true);
      }

      journalPages = await listJournalPages(10, 0);
      hasMore = journalPages.length >= 10;
    } catch (e) {
      console.error("Failed to load journals:", e);
    }
    loading = false;
  }

  async function loadMore() {
    if (loadingMore || !hasMore) return;
    loadingMore = true;
    try {
      const more = await listJournalPages(10, journalPages.length);
      if (more.length < 10) hasMore = false;
      journalPages = [...journalPages, ...more];
    } catch (e) {
      console.error("Failed to load more journals:", e);
    }
    loadingMore = false;
  }
</script>

<div class="journal-view">
  {#if loading}
    <div class="loading">Loading journals...</div>
  {:else}
    {#each journalPages as page (page.id)}
      <div class="journal-entry">
        <PageContent {page} compact />
      </div>
      <hr class="journal-divider" />
    {/each}

    {#if loadingMore}
      <div class="loading-more">Loading more...</div>
    {/if}

    {#if hasMore && !loadingMore}
      <button class="load-more-btn" onclick={loadMore}>Load older journals</button>
    {/if}
  {/if}
</div>

<style>
  .journal-view {
    height: 100%;
    padding: var(--content-padding-y, 40px) var(--content-padding-x, 24px);
  }

  .journal-entry {
    margin-bottom: 0;
  }

  .journal-divider {
    border: none;
    border-top: 1px solid var(--border);
    margin: 12px 0;
  }

  .loading, .loading-more {
    color: var(--text-secondary);
    padding: 24px;
    text-align: center;
  }

  .load-more-btn {
    display: block;
    margin: 16px auto;
    padding: 8px 16px;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .load-more-btn:hover {
    background: var(--bg-active);
    color: var(--text-primary);
  }
</style>
