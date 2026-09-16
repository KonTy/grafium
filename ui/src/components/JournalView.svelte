<script lang="ts">
  import { onDestroy, setContext, tick, untrack } from "svelte";
  import KeyboardSelectionToolbar from "./KeyboardSelectionToolbar.svelte";
  import { createKeyboardBlockSelection, KEYBOARD_BLOCK_SELECTION, type SelectionDirection } from "../lib/keyboardBlockSelection.svelte";
  import { SvelteSet } from "svelte/reactivity";
  import PageContent from "./PageContent.svelte";
  import DatePicker from "./DatePicker.svelte";
  import { listJournalPages, listJournalNoteDates, createPage, getPage, deletePage } from "../lib/api";
  import type { Page } from "../lib/api";
  import { contextMenuPositionFromEvent } from "../lib/contextMenu";
  import { dispatchEditPageEnd } from "../lib/editorInsert";
  import { MAIN_PANE_SCROLL_INTENT } from "../lib/mainPaneScroll";
  import { showToast } from "../lib/toast.svelte";
  import { formatLocalIsoDate, insertJournalPageByTitleDesc, isJournalDateTitle } from "../lib/journalDate";
  import { isPageNotFoundError } from "../lib/navigation";

  interface Props {
    openCalendar?: boolean;
    onCalendarOpened?: () => void;
    onGoToLink?: () => void;
    restorePageTitle?: string;
    restoreRequestId?: number;
    editTodayRequestId?: number;
    onNavigate?: (target: string) => void;
    onActivePageChange?: (page: Page | null) => void;
    onPageDeleted?: () => void;
    showBlockGuides?: boolean;
  }

  let {
    openCalendar = false,
    onCalendarOpened,
    onGoToLink,
    restorePageTitle = "",
    restoreRequestId = 0,
    editTodayRequestId = 0,
    onNavigate,
    onActivePageChange,
    onPageDeleted,
    showBlockGuides = true,
  }: Props = $props();

  let journalPages: Page[] = $state([]);
  let loading = $state(true);
  let loadingMore = $state(false);
  let hasMore = $state(true);
  let bottomSentinel: HTMLDivElement | null = $state(null);
  let journalFeedEl: HTMLDivElement | null = $state(null);
  let loadError = $state("");
  let moreError = $state("");
  let dateError = $state("");
  let refreshing = $state(false);
  let destroyed = false;
  let loadRequest = 0;
  const mountedPages = new SvelteSet<string>();
  const pendingPages = new SvelteSet<string>();
  const entryNodes = new Map<string, HTMLElement>();
  const nearbyPages = new Set<string>();
  let mountObserver: IntersectionObserver | null = null;
  let hydrateFrame = 0;
  let sentinelNearby = false;
  let navigationTarget: string | null = null;
  let scrollIntent = 0;
  type ScrollAnchor = { node: HTMLElement; top: number; intent: number };
  const pendingAnchors = new Map<string, ScrollAnchor>();
  const pageEditors: Record<string, PageContent | undefined> = {};
  type BoundaryNavigation = {
    pageId: string;
    direction: "up" | "down";
    caretX: number;
    source: Element;
    selection?: { isCurrent: () => boolean; resolve: (pageId: string | null) => void };
  };
  let boundaryNavigation = $state<BoundaryNavigation | null>(null);
  let focusingBoundary = $state(false);
  const keyboardSelection = createKeyboardBlockSelection(() => journalFeedEl?.parentElement ?? null, selectAdjacentPage);
  setContext(KEYBOARD_BLOCK_SELECTION, keyboardSelection);
  onDestroy(() => {
    cancelBoundaryNavigation();
    keyboardSelection.destroy();
  });

  function cancelBoundaryNavigation() {
    boundaryNavigation?.selection?.resolve(null);
    boundaryNavigation = null;
  }

  function selectAdjacentPage(pageId: string, direction: SelectionDirection, isCurrent: () => boolean): Promise<string | null> {
    const source = document.activeElement;
    if (!source || !isCurrent()) return Promise.resolve(null);
    cancelBoundaryNavigation();
    scrollIntent += 1;
    navigationTarget = null;
    if (direction === "down" && journalPages.at(-1)?.id === pageId) moreError = "";
    return new Promise((resolve) => {
      boundaryNavigation = { pageId, direction, caretX: 0, source, selection: { isCurrent, resolve } };
    });
  }

  function boundaryIsCurrent(request: BoundaryNavigation): boolean {
    return !destroyed && boundaryNavigation === request
      && request.source.isConnected && document.activeElement === request.source
      && (!request.selection || request.selection.isCurrent());
  }

  function navigateJournalBoundary(pageId: string, direction: "up" | "down", caretX: number) {
    const source = document.activeElement;
    if (!source || !journalFeedEl?.contains(source)) return;
    if (boundaryNavigation?.pageId === pageId && boundaryNavigation.direction === direction
      && boundaryIsCurrent(boundaryNavigation)) return;
    cancelBoundaryNavigation();
    scrollIntent += 1;
    navigationTarget = null;
    if (direction === "down" && journalPages.at(-1)?.id === pageId) moreError = "";
    boundaryNavigation = { pageId, direction, caretX, source };
  }

  async function focusJournalBoundary(request: BoundaryNavigation, target: Page) {
    focusingBoundary = true;
    try {
      if (request.selection) {
        request.selection.resolve(target.id);
        return;
      }
      const editor = pageEditors[target.id];
      if (!editor) throw new Error("Journal editor is not ready");
      await editor.focusForNav(request.caretX, request.direction === "down" ? "top" : "bottom",
        () => boundaryIsCurrent(request));
    } catch (error) {
      if (!boundaryIsCurrent(request)) return;
      const message = `Could not move to ${target.title}: ${String(error)}`;
      console.error(message);
      showToast(message, "error");
    } finally {
      focusingBoundary = false;
      if (boundaryNavigation === request) cancelBoundaryNavigation();
    }
  }

  $effect(() => {
    const request = boundaryNavigation;
    if (!request || loading || loadingMore || goingToDate || refreshing || focusingBoundary) return;
    if (!boundaryIsCurrent(request) || loadError) {
      cancelBoundaryNavigation();
      return;
    }
    const index = journalPages.findIndex((page) => page.id === request.pageId);
    const target = index < 0 ? undefined : journalPages[index + (request.direction === "down" ? 1 : -1)];
    if (!target) {
      if (index >= 0 && request.direction === "down" && hasMore && !moreError) {
        if (!pendingPages.size) untrack(() => void loadMore());
      } else {
        cancelBoundaryNavigation();
      }
      return;
    }
    if (!mountedPages.has(target.id)) {
      untrack(() => mountPage(target.id, false));
      return;
    }
    if (pendingPages.has(target.id)) return;
    untrack(() => void focusJournalBoundary(request, target));
  });

  $effect(() => {
    // A slow entry must not steal focus after the user types, clicks, or opens a dialog.
    const cancel = cancelBoundaryNavigation;
    const onKeydown = (event: KeyboardEvent) => {
      const request = boundaryNavigation;
      if (request && event.shiftKey === !!request.selection && !event.altKey && !event.ctrlKey && !event.metaKey
        && event.key === (request.direction === "down" ? "ArrowDown" : "ArrowUp")) return;
      cancel();
    };
    const onFocus = () => {
      if (boundaryNavigation && !boundaryIsCurrent(boundaryNavigation)) cancel();
    };
    window.addEventListener("keydown", onKeydown, true);
    window.addEventListener("pointerdown", cancel, true);
    window.addEventListener("input", cancel, true);
    window.addEventListener("focusin", onFocus);
    return () => {
      window.removeEventListener("keydown", onKeydown, true);
      window.removeEventListener("pointerdown", cancel, true);
      window.removeEventListener("input", cancel, true);
      window.removeEventListener("focusin", onFocus);
    };
  });

  function captureScrollAnchor(): ScrollAnchor | undefined {
    if (!journalFeedEl) return;
    if (journalFeedEl.scrollTop === 0
      && !journalFeedEl.contains(document.activeElement?.closest(".cm-editor") ?? null)) return;
    const top = journalFeedEl.getBoundingClientRect().top;
    for (const page of journalPages) {
      const node = entryNodes.get(page.id);
      if (node && node.getBoundingClientRect().bottom > top) {
        return { node, top: node.getBoundingClientRect().top, intent: scrollIntent };
      }
    }
  }

  function restoreScrollAnchor(anchor: ScrollAnchor | undefined) {
    if (!anchor?.node.isConnected || !journalFeedEl) return;
    // Native anchoring may have corrected some or all of the height change.
    // Only correct the remainder, unless the user moved since this request.
    if (anchor.intent !== scrollIntent) return;
    const scale = journalFeedEl.getBoundingClientRect().height / journalFeedEl.offsetHeight || 1;
    journalFeedEl.scrollTop += (anchor.node.getBoundingClientRect().top - anchor.top) / scale;
  }

  function noteScrollIntent() {
    scrollIntent += 1;
    navigationTarget = null;
    cancelBoundaryNavigation();
  }

  function trackScrollIntent(node: HTMLElement) {
    const onKeydown = (event: KeyboardEvent) => {
      if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(event.key)) noteScrollIntent();
    };
    node.addEventListener("wheel", noteScrollIntent, { passive: true });
    node.addEventListener("pointerdown", noteScrollIntent, { passive: true });
    node.addEventListener("keydown", onKeydown);
    node.addEventListener(MAIN_PANE_SCROLL_INTENT, noteScrollIntent);
    return {
      destroy() {
        node.removeEventListener("wheel", noteScrollIntent);
        node.removeEventListener("pointerdown", noteScrollIntent);
        node.removeEventListener("keydown", onKeydown);
        node.removeEventListener(MAIN_PANE_SCROLL_INTENT, noteScrollIntent);
      },
    };
  }

  // Once mounted, an editor stays mounted: scrolling must not discard its
  // CodeMirror state, pending save, selection, collapsed blocks or undo history.
  function mountPage(pageId: string, preserveScroll = true) {
    if (mountedPages.has(pageId)) return;
    const node = entryNodes.get(pageId);
    const changesContentAboveViewport = node && journalFeedEl
      && node.getBoundingClientRect().top < journalFeedEl.getBoundingClientRect().top;
    const anchor = preserveScroll && changesContentAboveViewport ? captureScrollAnchor() : undefined;
    if (anchor) pendingAnchors.set(pageId, anchor);
    pendingPages.add(pageId);
    mountedPages.add(pageId);
  }

  function scheduleHydration() {
    if (destroyed || hydrateFrame) return;
    hydrateFrame = requestAnimationFrame(() => {
      hydrateFrame = 0;
      if (loading || goingToDate || pendingPages.size || !journalFeedEl) return;
      const feed = journalFeedEl.getBoundingClientRect();
      for (const page of journalPages) {
        if (mountedPages.has(page.id) || !nearbyPages.has(page.id)) continue;
        const node = entryNodes.get(page.id);
        if (!node) continue;
        // Intersection records can predate the preceding entry's content.
        // Recheck after layout, and hydrate only one entry at a time.
        const rect = node.getBoundingClientRect();
        if (rect.bottom >= feed.top - 250 && rect.top <= feed.bottom + 250) {
          mountPage(page.id);
          return;
        }
      }
      if (sentinelNearby && !moreError && bottomSentinel
        && bottomSentinel.getBoundingClientRect().top <= feed.bottom + 250) void loadMore();
    });
  }

  async function pageLoadSettled(page: Page) {
    pendingPages.delete(page.id);
    await tick();
    if (destroyed) return;
    restoreScrollAnchor(pendingAnchors.get(page.id));
    pendingAnchors.delete(page.id);
    if (navigationTarget === page.title) {
      navigationTarget = null;
      entryNodes.get(page.id)?.scrollIntoView({ block: "start" });
    }
    scheduleHydration();
  }

  function trackMount(node: HTMLElement, pageId: string) {
    entryNodes.set(pageId, node);
    if (!mountObserver) {
      mountObserver = new IntersectionObserver((entries) => {
        for (const entry of entries) {
          const id = (entry.target as HTMLElement).dataset.pageId!;
          if (entry.isIntersecting) nearbyPages.add(id);
          else nearbyPages.delete(id);
        }
        scheduleHydration();
      }, { root: journalFeedEl, rootMargin: "250px 0px" });
    }
    node.dataset.pageId = pageId;
    mountObserver.observe(node);
    return {
      destroy() {
        mountObserver?.unobserve(node);
        entryNodes.delete(pageId);
        nearbyPages.delete(pageId);
        pendingPages.delete(pageId);
        pendingAnchors.delete(pageId);
      },
    };
  }

  onDestroy(() => {
    destroyed = true;
    loadRequest += 1;
    cancelAnimationFrame(hydrateFrame);
    mountObserver?.disconnect();
    visibilityObserver?.disconnect();
  });

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
  let goToDateButton: HTMLButtonElement | null = $state(null);
  let calendarSelectedDate = $state(formatLocalIsoDate());
  let calendarNoteDates: string[] = $state([]);
  let calendarNotesRequest = 0;
  let goingToDate = $state(false);
  let lastEditTodayHandled = 0;
  let lastRestoreHandled = "";

  $effect(() => {
    const requestId = editTodayRequestId;
    if (!requestId || loading || goingToDate || requestId === lastEditTodayHandled) return;
    lastEditTodayHandled = requestId;
    const today = getLocalDate();
    untrack(() => {
      void goToJournalDate(today).then(() => {
        if (destroyed) return;
        // Focus owns the final scroll position. If blocks are still loading,
        // PageContent consumes this pending request when they arrive.
        navigationTarget = null;
        dispatchEditPageEnd({ pageTitle: today });
      });
    });
  });

  $effect(() => {
    void loadJournals();

    // Check every few seconds for midnight rollover and external journal updates.
    const interval = setInterval(async () => {
      const now = getLocalDate();
      if (now !== lastDate) {
        await refreshVisibleJournals(true);
        return;
      }

      // Passive refresh so externally indexed journals appear without manual reindex or navigation.
      await refreshVisibleJournals();
    }, 3_000);

    return () => clearInterval(interval);
  });

  $effect(() => {
    const title = restorePageTitle;
    const request = `${restoreRequestId}:${title}`;
    if (!loading && !goingToDate && title && request !== lastRestoreHandled) {
      lastRestoreHandled = request;
      untrack(() => void goToJournalDate(title));
    }
  });

  $effect(() => {
    if (!bottomSentinel) return;

    const root = journalFeedEl ?? bottomSentinel.closest(".journal-feed") ?? bottomSentinel.closest(".main-content");
    const observer = new IntersectionObserver(
      (entries) => {
        sentinelNearby = entries.some((entry) => entry.isIntersecting);
        if (sentinelNearby) scheduleHydration();
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
    const request = ++loadRequest;
    loading = true;
    loadError = "";
    try {
      // Ensure today's journal exists
      const today = getLocalDate();
      await getOrCreateJournalPage(today);

      const loadedPages = await listJournalPages(10, 0);
      if (destroyed || request !== loadRequest) return;
      journalPages = loadedPages;
      hasMore = loadedPages.length >= 10;
      if (!restorePageTitle && loadedPages[0]) mountPage(loadedPages[0].id);
    } catch (e) {
      if (destroyed || request !== loadRequest) return;
      loadError = `Could not load journals: ${String(e)}`;
      console.error("Failed to load journals:", e);
    } finally {
      if (!destroyed && request === loadRequest) {
        loading = false;
        scheduleHydration();
      }
    }
  }

  async function loadMore() {
    if (loading || loadError || loadingMore || goingToDate || refreshing || pendingPages.size || !hasMore) return;
    const request = loadRequest;
    loadingMore = true;
    moreError = "";
    try {
      const more = await listJournalPages(10, journalPages.length);
      if (destroyed || request !== loadRequest) return;
      if (more.length < 10) hasMore = false;
      for (const page of more) journalPages = insertJournalPageByTitleDesc(journalPages, page);
    } catch (e) {
      if (destroyed || request !== loadRequest) return;
      moreError = `Could not load older journals: ${String(e)}`;
      console.error("Failed to load more journals:", e);
    } finally {
      loadingMore = false;
    }
  }

  $effect(() => {
    if (openCalendar && goToDateButton && !loading && !goingToDate) {
      untrack(() => {
        openGoToDatePicker();
        onCalendarOpened?.();
      });
    }
  });

  function openGoToDatePicker() {
    if (!goToDateButton) return;
    const rect = goToDateButton.getBoundingClientRect();
    const top = journalFeedEl?.getBoundingClientRect().top ?? 0;
    calendarSelectedDate = journalPages.find((page) => {
      const node = entryNodes.get(page.id);
      return node && node.getBoundingClientRect().bottom > top;
    })?.title ?? formatLocalIsoDate();
    calendarNoteDates = [];
    calendarNotesRequest += 1;
    goToDatePicker = { x: rect.right - 250, y: rect.bottom + 6 };
  }

  async function loadCalendarNoteDates(year: number, month: number) {
    const request = ++calendarNotesRequest;
    calendarNoteDates = [];
    try {
      const dates = await listJournalNoteDates(year, month);
      if (!destroyed && goToDatePicker && request === calendarNotesRequest) calendarNoteDates = dates;
    } catch (error) {
      if (destroyed || !goToDatePicker || request !== calendarNotesRequest) return;
      const message = `Could not load calendar note markers: ${String(error)}`;
      console.error(message);
      showToast(message, "error");
    }
  }

  async function scrollToJournalDate(page: Page) {
    scrollIntent += 1;
    navigationTarget = page.title;
    mountPage(page.id, false);
    await tick();
    if (destroyed) return;
    entryNodes.get(page.id)?.scrollIntoView({ block: "start" });
    if (!pendingPages.has(page.id)) navigationTarget = null;
    onActivePageChange?.(page);
  }

  async function goToJournalDate(title: string) {
    goToDatePicker = null;
    if (!isJournalDateTitle(title) || goingToDate) return;
    goingToDate = true;
    loadRequest += 1;
    dateError = "";
    try {
      const page = await getOrCreateJournalPage(title);
      if (destroyed) return;

      const alreadyLoaded = journalPages.some((item) => item.id === page.id || item.title === page.title);
      if (alreadyLoaded) {
        journalPages = insertJournalPageByTitleDesc(journalPages, page);
        await scrollToJournalDate(page);
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
        await scrollToJournalDate(page);
        return;
      }

      const pageSize = 10;
      let loaded = journalPages;
      let moreAvailable = hasMore;
      while (!loaded.some((item) => item.title === page.title) && moreAvailable) {
        const more = await listJournalPages(pageSize, loaded.length);
        if (destroyed) return;
        loaded = [...loaded, ...more];
        moreAvailable = more.length >= pageSize;
        const last = loaded[loaded.length - 1]?.title;
        if (last && last < page.title) break;
      }
      journalPages = insertJournalPageByTitleDesc(loaded, page);
      hasMore = moreAvailable;
      await scrollToJournalDate(page);
    } catch (e) {
      dateError = `Could not open ${title}: ${String(e)}`;
      console.error("Failed to open journal date:", e);
    } finally {
      goingToDate = false;
      scheduleHydration();
    }
  }

  async function getOrCreateJournalPage(title: string): Promise<Page> {
    try {
      return await getPage({ title });
    } catch (error) {
      // A locked/unavailable database is not a missing journal.
      if (!isPageNotFoundError(error)) {
        throw error;
      }
      return createPage(title, true);
    }
  }

  async function refreshVisibleJournals(ensureToday = false) {
    if (loading || loadingMore || goingToDate || refreshing || journalPages.length === 0) return;
    refreshing = true;
    const request = loadRequest;

    try {
      if (ensureToday) {
        const today = getLocalDate();
        await getOrCreateJournalPage(today);
        lastDate = today;
      }
      // Refresh currently visible slice; preserves scroll and picks up external edits/newer pages.
      const fresh = await listJournalPages(journalPages.length + (ensureToday ? 1 : 0), 0);
      if (destroyed || request !== loadRequest) return;
      const anchor = captureScrollAnchor();
      if (fresh.length > 0 && pagesChanged(journalPages, fresh)) {
        // A newly indexed day can push the oldest mounted editor outside the
        // metadata slice. Keep it (and its pending edits) until explicit deletion.
        const retained = journalPages.filter((page) => mountedPages.has(page.id)
          && !fresh.some((item) => item.id === page.id));
        journalPages = [...fresh, ...retained].sort((a, b) => b.title.localeCompare(a.title));
        await tick();
        restoreScrollAnchor(anchor);
      }

      // Recompute whether more journals exist beyond the loaded slice.
      const next = await listJournalPages(1, journalPages.length);
      if (destroyed || request !== loadRequest) return;
      hasMore = next.length > 0;
    } catch (e) {
      console.error("Failed to refresh journals:", e);
    } finally {
      refreshing = false;
    }
  }
</script>

<div class="journal-view" data-keyboard-block-selection={keyboardSelection.active ? "true" : undefined}>
  <div class="journal-toolbar">
    <button
      bind:this={goToDateButton}
      class="journal-nav-btn goto-date-btn"
      type="button"
      aria-label="Go to date"
      title="Go to date (Ctrl/Cmd+G)"
      aria-haspopup="dialog"
      aria-expanded={goToDatePicker !== null}
      disabled={loading || goingToDate}
      onclick={openGoToDatePicker}
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect x="3" y="5" width="18" height="16" rx="2" />
        <path d="M16 3v4M8 3v4M3 11h18" />
      </svg>
    </button>
    <button
      class="journal-nav-btn"
      type="button"
      aria-label="Go to link"
      title="Go to link (Ctrl/Cmd+L)"
      aria-haspopup="dialog"
      onclick={onGoToLink}
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M10 13a5 5 0 0 0 7 .2l3-3a5 5 0 0 0-7-7l-1.7 1.7M14 11a5 5 0 0 0-7-.2l-3 3a5 5 0 0 0 7 7l1.7-1.7" />
      </svg>
    </button>
  </div>
  <KeyboardSelectionToolbar selection={keyboardSelection} />
  <div class="journal-feed" data-main-scroll-pane bind:this={journalFeedEl} use:trackScrollIntent
    role="region" aria-label="Journal entries">
    {#if loading && journalPages.length === 0}
      <div class="loading" role="status">Loading journals...</div>
    {/if}
    {#if loadError}
      <div class="journal-error" role="alert">
        {loadError}
        <button type="button" onclick={loadJournals}>Retry loading journals</button>
      </div>
    {/if}
    {#if dateError}
      <div class="journal-error" role="alert">{dateError} Try Go to date again.</div>
    {/if}
      {#each journalPages as page (page.id)}
        <div
          class="journal-entry"
          id={`journal-page-${page.title}`}
          data-page-title={page.title}
          class:journal-placeholder={!mountedPages.has(page.id) || pendingPages.has(page.id)}
          aria-busy={pendingPages.has(page.id)}
          oncontextmenu={(e) => handleDateRightClick(e, page)}
          use:trackVisibility={page.id}
          use:trackMount={page.id}
        >
          {#if mountedPages.has(page.id)}
            <PageContent
              bind:this={pageEditors[page.id]}
              {page}
              compact
              {showBlockGuides}
              onLoadSettled={() => pageLoadSettled(page)}
              onNavigateBoundary={(direction, caretX) => navigateJournalBoundary(page.id, direction, caretX)}
            />
            {#if pendingPages.has(page.id)}
              <div class="entry-loading" role="status">Loading entry...</div>
            {/if}
          {:else}
            <div class="deferred-entry">
              <h1 class="page-title">{page.title.replace(/_/g, "-")}</h1>
              <button type="button" onclick={() => mountPage(page.id)}>Load entry</button>
            </div>
          {/if}
        </div>
        <hr class="journal-divider" />
      {/each}

      {#if loadingMore}
        <div class="loading-more" role="status">Loading more...</div>
      {/if}
      {#if moreError}
        <div class="journal-error" role="alert">{moreError}</div>
      {/if}

      {#if hasMore && !loadingMore && !loadError}
        <button class="load-more-btn" disabled={loading || goingToDate || pendingPages.size > 0} onclick={loadMore}>
          {moreError ? "Retry loading older journals" : "Load older journals"}
        </button>

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
      selectedDate={calendarSelectedDate}
      markedDates={calendarNoteDates}
      onMonthChange={loadCalendarNoteDates}
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
    gap: 6px;
    padding: 8px 12px 6px;
    background: var(--bg-primary);
  }

  .journal-feed {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .journal-nav-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-hover);
    color: var(--text-primary);
    cursor: pointer;
    font-size: 13px;
  }

  .journal-nav-btn:hover:not(:disabled) {
    background: var(--bg-active);
  }

  .journal-nav-btn:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  @media (pointer: coarse) {
    .journal-nav-btn { width: 44px; height: 44px; }
  }

  .journal-nav-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .journal-entry {
    margin: 0 0 4px;
  }

  .journal-placeholder {
    min-height: 360px;
  }

  .deferred-entry {
    padding: 20px 24px;
  }

  .deferred-entry .page-title {
    margin: 0 0 16px;
    color: var(--accent);
    font-size: 22px;
    line-height: 1.2;
  }

  .deferred-entry button, .journal-error button {
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-hover);
    color: var(--text-primary);
    cursor: pointer;
  }

  .entry-loading, .journal-error {
    padding: 12px 24px;
    color: var(--text-secondary);
  }

  .journal-entry :global(.page-content.compact .page-heading) {
    margin-bottom: 10px;
    padding-bottom: 6px;
  }

  .journal-entry :global(.page-content.compact .page-title) {
    color: var(--accent);
    font-size: 22px;
    font-weight: 700;
    line-height: 1.2;
    letter-spacing: 0.01em;
    overflow-wrap: normal;
    word-break: keep-all;
  }

  @media (max-width: 640px) {
    .journal-view {
      padding-bottom: 64px;
    }

    .journal-entry :global(.page-content.compact .page-title) {
      font-size: 1.25rem;
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
