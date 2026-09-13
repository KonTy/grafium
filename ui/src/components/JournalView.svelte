<script lang="ts">
  import PageContent from "./PageContent.svelte";
  import DatePicker from "./DatePicker.svelte";
  import { listJournalPages, createPage, getPage, deletePage } from "../lib/api";
  import type { Page } from "../lib/api";
  import { contextMenuPositionFromEvent } from "../lib/contextMenu";
  import { dispatchEditPageEnd } from "../lib/editorInsert";
  import { formatLocalIsoDate, insertJournalPageByTitleDesc, isJournalDateTitle } from "../lib/journalDate";

  interface Props {
    restorePageTitle?: string;
    restoreRequestId?: number;
    editTodayRequestId?: number;
    onNavigate?: (target: string) => void;
    onActivePageChange?: (page: Page | null) => void;
    onPageDeleted?: () => void;
  }

  let {
    restorePageTitle = "",
    restoreRequestId = 0,
    editTodayRequestId = 0,
    onNavigate,
    onActivePageChange,
    onPageDeleted,
  }: Props = $props();

  let journalPages: Page[] = $state([]);
  let loading = $state(true);
  let loadingMore = $state(false);
  let hasMore = $state(true);
  let bottomSentinel: HTMLDivElement | null = $state(null);
  let journalFeedEl: HTMLDivElement | null = $state(null);

  // Journals are a scrolling feed of many day-pages rather than one "current page", so the
  // Reference/Knowledge panel needs to know which entry is actually in view to enable
  // "Summarize this Page" / "Summarize Selection" while browsing the journal.
  const visibilityRatios = new Map<string, number>();

  function reportMostVisibleEntry() {
    let bestId: string | null = null;
    let bestRatio = 0;
    for (const [id, ratio] of visibilityRatios) {
      if (ratio > bestRatio) {
        bestRatio = ratio;
        bestId = id;
      }
    }
    const active = bestId ? journalPages.find((page) => page.id === bestId) ?? null : null;
    onActivePageChange?.(active);
  }

  interface ContextMenu {
    x: number;
    y: number;
    page: Page;
  }

  let contextMenu: ContextMenu | null = $state(null);

  function pagesChanged(a: Page[], b: Page[]): boolean {
    if (a.length !== b.length) return true;
    return a.some((page, index) => {
      const other = b[index];
      return !other
        || page.id !== other.id
        || page.title !== other.title
        || page.file_path !== other.file_path
        || page.updated_at !== other.updated_at
        || page.is_journal !== other.is_journal;
    });
  }

  function getLocalDate(): string {
    return formatLocalIsoDate();
  }

  let lastDate = getLocalDate();
  let goToDatePicker: { x: number; y: number } | null = $state(null);
  let goingToDate = $state(false);
  let lastEditTodayHandled = 0;

  $effect(() => {
    const requestId = editTodayRequestId;
    if (!requestId || loading || requestId === lastEditTodayHandled) return;
    lastEditTodayHandled = requestId;
    const today = getLocalDate();
    const tryFocus = (attempt = 0) => {
      const el = document.getElementById(`journal-page-${today}`);
      if (!el && attempt < 40) {
        requestAnimationFrame(() => tryFocus(attempt + 1));
        return;
      }
      el?.scrollIntoView({ block: "start" });
      dispatchEditPageEnd({ pageTitle: today });
    };
    tryFocus();
  });

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

    const root = journalFeedEl ?? bottomSentinel.closest(".journal-feed") ?? bottomSentinel.closest(".main-content");
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

  let visibilityObserver: IntersectionObserver | null = null;

  /** Svelte action: tracks how much of a journal-entry is on-screen so we can report the
   * "most visible" one as the active page for the Reference/Knowledge panel. */
  function trackVisibility(node: HTMLElement, pageId: string) {
    if (!visibilityObserver) {
      const root = journalFeedEl ?? node.closest(".journal-feed") ?? node.closest(".main-content");
      visibilityObserver = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            const id = (entry.target as HTMLElement).dataset.pageId;
            if (!id) continue;
            visibilityRatios.set(id, entry.isIntersecting ? entry.intersectionRatio : 0);
          }
          reportMostVisibleEntry();
        },
        { root, threshold: [0, 0.1, 0.25, 0.5, 0.75, 1] }
      );
    }

    node.dataset.pageId = pageId;
    visibilityObserver.observe(node);

    return {
      destroy() {
        visibilityObserver?.unobserve(node);
        visibilityRatios.delete(pageId);
        reportMostVisibleEntry();
      },
    };
  }

  $effect(() => {
    function closeMenu() {
      contextMenu = null;
    }

    window.addEventListener("click", closeMenu);
    window.addEventListener("contextmenu", closeMenu);

    return () => {
      window.removeEventListener("click", closeMenu);
      window.removeEventListener("contextmenu", closeMenu);
    };
  });

  function handleDateRightClick(e: MouseEvent, page: Page) {
    const target = e.target as HTMLElement | null;
    if (!target?.closest(".page-title")) return;

    e.preventDefault();
    e.stopPropagation();
    contextMenu = { ...contextMenuPositionFromEvent(e, { width: 190, height: 100 }), page };
  }

  async function handleDeletePage() {
    if (!contextMenu) return;
    const pageToDelete = contextMenu.page;
    contextMenu = null;

    const confirmed = window.confirm(`Delete journal page '${pageToDelete.title}'? This will delete the .md file from disk.`);
    if (!confirmed) return;

    try {
      await deletePage(pageToDelete.id);
      journalPages = journalPages.filter((page) => page.id !== pageToDelete.id);

      const next = await listJournalPages(1, journalPages.length);
      hasMore = next.length > 0;
      // Ask App.svelte to refresh the sidebar's Favorites + Recent Pages
      // so the just-deleted journal entry doesn't linger there.
      onPageDeleted?.();
    } catch (e) {
      console.error("Failed to delete journal page:", e);
      alert("Failed to delete journal page.");
    }
  }

  function handleImportMediaClick() {
    contextMenu = null;
    onNavigate?.("__import_media_journal__");
  }

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

      if (restorePageTitle && !journalPages.some((page) => page.title === restorePageTitle)) {
        try {
          await getPage({ title: restorePageTitle });
        } catch {
          await createPage(restorePageTitle, true);
        }
        journalPages = await listJournalPages(Math.max(pageSize, journalPages.length + 1), 0);
      }
    } catch (e) {
      console.error("Failed to load journals:", e);
    }
    loading = false;
    if (restorePageTitle) {
      requestAnimationFrame(() => {
        document.getElementById(`journal-page-${restorePageTitle}`)?.scrollIntoView({ block: "start" });
      });
    }
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

  function openGoToDatePicker(event: MouseEvent) {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    goToDatePicker = { x: rect.right - 250, y: rect.bottom + 6 };
  }

  function scrollToJournalDate(title: string) {
    requestAnimationFrame(() => {
      document.getElementById(`journal-page-${title}`)?.scrollIntoView({ block: "start" });
    });
  }

  async function goToJournalDate(title: string) {
    goToDatePicker = null;
    if (!isJournalDateTitle(title) || goingToDate) return;
    goingToDate = true;
    try {
      let page: Page;
      try {
        page = await getPage({ title });
      } catch {
        page = await createPage(title, true);
      }

      const alreadyLoaded = journalPages.some((item) => item.id === page.id || item.title === page.title);
      if (alreadyLoaded) {
        journalPages = insertJournalPageByTitleDesc(journalPages, page);
        scrollToJournalDate(page.title);
        return;
      }

      const newest = journalPages[0]?.title;
      const oldest = journalPages[journalPages.length - 1]?.title;
      if (
        journalPages.length === 0
        || (newest && page.title >= newest)
        || (oldest && page.title >= oldest)
      ) {
        journalPages = insertJournalPageByTitleDesc(journalPages, page);
        scrollToJournalDate(page.title);
        return;
      }

      const pageSize = 10;
      let loaded = journalPages;
      let moreAvailable = hasMore;
      while (!loaded.some((item) => item.title === page.title) && moreAvailable) {
        const more = await listJournalPages(pageSize, loaded.length);
        loaded = [...loaded, ...more];
        moreAvailable = more.length >= pageSize;
        const last = loaded[loaded.length - 1]?.title;
        if (last && last < page.title) break;
      }
      journalPages = insertJournalPageByTitleDesc(loaded, page);
      hasMore = moreAvailable;
      scrollToJournalDate(page.title);
    } catch (e) {
      console.error("Failed to open journal date:", e);
    } finally {
      goingToDate = false;
    }
  }

  async function refreshVisibleJournals() {
    if (loading || loadingMore || journalPages.length === 0) return;

    try {
      // Refresh currently visible slice; preserves scroll and picks up external edits/newer pages.
      const fresh = await listJournalPages(journalPages.length, 0);
      if (fresh.length > 0 && pagesChanged(journalPages, fresh)) {
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
  <div class="journal-toolbar">
    <button
      class="goto-date-btn"
      type="button"
      aria-haspopup="dialog"
      aria-expanded={goToDatePicker !== null}
      disabled={loading || goingToDate}
      onclick={openGoToDatePicker}
    >
      Go to date
    </button>
  </div>
  <div class="journal-feed" bind:this={journalFeedEl}>
    {#if loading}
      <div class="loading">Loading journals...</div>
    {:else}
      {#each journalPages as page (page.id)}
        <div
          class="journal-entry"
          id={`journal-page-${page.title}`}
          data-page-title={page.title}
          oncontextmenu={(e) => handleDateRightClick(e, page)}
          use:trackVisibility={page.id}
        >
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

      {#if contextMenu}
        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
        <div
          class="context-menu app-context-menu"
          style="top:{contextMenu.y}px;left:{contextMenu.x}px;"
          onclick={(e) => e.stopPropagation()}
        >
          <button class="context-menu-item danger" onclick={handleDeletePage}>
            Delete page
          </button>
          <button class="context-menu-item" onclick={handleImportMediaClick}>
            Import from Media...
          </button>
        </div>
      {/if}
    {/if}
  </div>
  {#if goToDatePicker}
    <DatePicker
      x={goToDatePicker.x}
      y={goToDatePicker.y}
      showClear={false}
      onSelect={goToJournalDate}
      onCancel={() => (goToDatePicker = null)}
    />
  {/if}
</div>

<style>
  .journal-view {
    height: 100%;
    max-height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-sizing: border-box;
    padding: 0;
  }

  .journal-toolbar {
    flex: 0 0 auto;
    z-index: 6;
    display: flex;
    justify-content: flex-end;
    padding: 8px 12px 6px;
    background: var(--bg-primary);
  }

  .journal-feed {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .goto-date-btn {
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-hover);
    color: var(--text-primary);
    cursor: pointer;
    font-size: 13px;
  }

  .goto-date-btn:hover:not(:disabled) {
    background: var(--bg-active);
  }

  .goto-date-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .journal-entry {
    margin: 0 0 4px;
  }

  .journal-entry :global(.page-content.compact .page-heading) {
    margin-bottom: 10px;
    padding-bottom: 6px;
  }

  .journal-entry :global(.page-content.compact .page-title) {
    color: var(--accent);
    font-size: clamp(26px, 3.8vw, 42px);
    font-weight: 800;
    line-height: 1.05;
    letter-spacing: 0.04em;
    overflow-wrap: normal;
    word-break: keep-all;
  }

  @media (max-width: 640px) {
    .journal-view {
      padding-bottom: 64px;
    }

    .journal-entry :global(.page-content.compact .page-title) {
      font-size: 1.55rem;
      letter-spacing: 0;
    }
  }

  .journal-divider {
    border: none;
    height: 2px;
    margin: 22px 0 18px;
    background: linear-gradient(
      90deg,
      color-mix(in srgb, var(--accent) 72%, transparent),
      color-mix(in srgb, var(--border) 82%, transparent) 42%,
      transparent 100%
    );
    box-shadow: 0 0 10px color-mix(in srgb, var(--accent) 18%, transparent);
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

  .context-menu {
    position: fixed;
    z-index: 2147483000;
    min-width: 150px;
    padding: 6px;
  }

  .context-menu-item {
    display: block;
    width: 100%;
    border: none;
    background: transparent;
    color: var(--text-primary);
    text-align: left;
    padding: 8px 10px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 13px;
  }

  .context-menu-item:hover {
    background: var(--bg-hover);
  }

  .context-menu-item.danger {
    color: #f38ba8;
  }
</style>
