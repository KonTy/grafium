<script lang="ts">
  import PageContent from "./PageContent.svelte";
  import { listJournalPages, createPage, getPage } from "../lib/api";
  import type { Page } from "../lib/api";

  interface Props {
    restorePageTitle?: string;
    restoreRequestId?: number;
  }

  let { restorePageTitle = "", restoreRequestId = 0 }: Props = $props();

  let journalPages: Page[] = $state([]);
  let loading = $state(true);
  let loadingMore = $state(false);
  let hasMore = $state(true);
  let bottomSentinel: HTMLDivElement | null = $state(null);

  function getLocalDate(): string {
    const now = new Date();
    return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
  }

  let lastDate = getLocalDate();

  $effect(() => {
    restorePageTitle;
    restoreRequestId;
    loadJournals();

    // Check every few seconds for midnight rollover and external journal updates.
    const interval = setInterval(async () => {
      const now = getLocalDate();
      if (now !== lastDate) {
        lastDate = now;
        await loadJournals();
        return;
      }

      // Passive refresh so externally indexed journals appear without manual reindex or navigation.
      await refreshVisibleJournals();
    }, 3_000);

    return () => clearInterval(interval);
  });

  $effect(() => {
    if (!bottomSentinel) return;

    const root = bottomSentinel.closest(".main-content");
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          void loadMore();
        }
      },
      {
        root,
        rootMargin: "250px 0px",
        threshold: 0,
      }
    );

    observer.observe(bottomSentinel);
    return () => observer.disconnect();
  });

  async function loadJournals() {
    loading = true;
    try {
      // Ensure today's journal exists
      const today = getLocalDate();
      try {
        await getPage({ title: today });
      } catch {
        await createPage(today, true);
      }

      const pageSize = 10;
      let loadedPages = await listJournalPages(pageSize, 0);
      let offset = loadedPages.length;
      let moreAvailable = loadedPages.length >= pageSize;

      while (restorePageTitle && !loadedPages.some((page) => page.title === restorePageTitle) && moreAvailable) {
        const more = await listJournalPages(pageSize, offset);
        loadedPages = [...loadedPages, ...more];
        offset += more.length;
        moreAvailable = more.length >= pageSize;
      }

      journalPages = loadedPages;
      hasMore = moreAvailable;
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

  async function refreshVisibleJournals() {
    if (loading || loadingMore || journalPages.length === 0) return;

    try {
      // Refresh currently visible slice; preserves scroll and picks up external edits/newer pages.
      const fresh = await listJournalPages(journalPages.length, 0);
      if (fresh.length > 0) {
        journalPages = fresh;
      }

      // Recompute whether more journals exist beyond the loaded slice.
      const next = await listJournalPages(1, fresh.length);
      hasMore = next.length > 0;
    } catch (e) {
      console.error("Failed to refresh journals:", e);
    }
  }
</script>

<div class="journal-view">
  {#if loading}
    <div class="loading">Loading journals...</div>
  {:else}
    {#each journalPages as page (page.id)}
      <div class="journal-entry" id={`journal-page-${page.title}`} data-page-title={page.title}>
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

    <div class="journal-bottom-sentinel" bind:this={bottomSentinel} aria-hidden="true"></div>
  {/if}
</div>

<style>
  .journal-view {
    height: 100%;
    padding: 0;
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

  .journal-bottom-sentinel {
    height: 1px;
  }
</style>
