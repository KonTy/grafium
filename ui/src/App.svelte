<script lang="ts">
  import { tick } from "svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import PageContent from "./components/PageContent.svelte";
  import JournalView from "./components/JournalView.svelte";
  import AllPages from "./components/AllPages.svelte";
  import GraphView from "./components/GraphView.svelte";
  import Statistics from "./components/Statistics.svelte";
  import FlashcardReview from "./components/FlashcardReview.svelte";
  import ChatbotView from "./components/ChatbotView.svelte";
  import Settings from "./components/Settings.svelte";
  import TitleBar from "./components/TitleBar.svelte";
  import ReferencePanel from "./components/ReferencePanel.svelte";
  import { getPage, createPage, recordPageOpen, getAppTheme, getSmplosTheme, getGraphInfo, openGraph, validateGraph, createGraph, reindexCurrent, listGraphs, mediaImportVideo, type GraphInfo } from "./lib/api";
  import { keymap_manager, registerDefaultShortcuts } from "./lib/keymap";
  import type { PageNavigationTarget } from "./lib/navigation";
  import { resolvePageLookup } from "./lib/navigation";
  import { applyTheme, getThemeById } from "./lib/themes";
  import { attachAppUndoRedoListeners } from "./lib/undoEvents";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import type { Page } from "./lib/api";

  /** Pick a folder using native OS dialog on all platforms */
  async function pickFolder(): Promise<string | null> {
    // Check if Android JS bridge is available
    if ((window as any).FolderPickerBridge) {
      return new Promise<string | null>((resolve) => {
        (window as any).__FOLDER_PICKER_RESOLVE = (result: string | null) => {
          delete (window as any).__FOLDER_PICKER_RESOLVE;
          resolve(result);
        };
        (window as any).FolderPickerBridge.pickFolder();
      });
    }
    // Desktop: use Tauri dialog plugin
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Select Folder",
    });
    if (selected && typeof selected === "string") {
      return selected;
    }
    return null;
  }

  type View = "page" | "journal" | "all-pages" | "flashcards" | "statistics" | "chat" | "settings" | "graph";

  let currentView: View = $state("page");
  let currentPage: Page | null = $state(null);
  let loading = $state(true);
  let error: string | null = $state(null);
  let sidebarVisible = $state(true);
  let sidebarWidth = $state(260);
  let isResizingSidebar = $state(false);
  let appLayoutEl: HTMLDivElement | null = null;
  let zenMode = $state(false);
  let referencePanelVisible = $state(false);
  let mainContentEl: HTMLElement | null = null;
  let restoreTimer: number | null = null;
  let pendingJournalRestore: HistoryEntry | null = $state(null);
  let journalRestoreRequestId = $state(0);
  let uiZoom = $state(1);

  // Navigation history for back/forward
  type HistoryEntry = {
    kind: View;
    title?: string;
    scrollTop: number;
    sourceBlockId?: string;
    sourcePageTitle?: string;
  };

  type LinkNavigateDetail = {
    pageName: string;
    sourceBlockId?: string;
    sourcePageTitle?: string;
    targetBlockId?: string;
  };

  let navHistory: HistoryEntry[] = $state([]);
  let navIndex = $state(-1);

  const DEFAULT_SIDEBAR_WIDTH = 260;
  const SIDEBAR_MIN_WIDTH = 180;
  const SIDEBAR_MAX_WIDTH = 520;
  const MAIN_CONTENT_MIN_WIDTH = 360;
  const DEFAULT_UI_ZOOM = 1;
  const MIN_UI_ZOOM = 0.7;
  const MAX_UI_ZOOM = 1.8;
  const UI_ZOOM_STEP = 0.05;

  function applyUiZoom(zoom: number) {
    uiZoom = zoom;
    document.documentElement.style.zoom = String(zoom);
  }

  function saveUiZoomPreference(zoom: number) {
    try {
      localStorage.setItem("grafium.ui.zoom", String(Math.round(zoom * 100)));
    } catch {
      // Ignore localStorage failures.
    }
  }

  function loadUiZoomPreference() {
    try {
      const raw = localStorage.getItem("grafium.ui.zoom");
      if (!raw) {
        applyUiZoom(DEFAULT_UI_ZOOM);
        return;
      }
      const percent = Number(raw);
      if (!Number.isFinite(percent)) {
        applyUiZoom(DEFAULT_UI_ZOOM);
        return;
      }
      const zoom = Math.max(MIN_UI_ZOOM, Math.min(MAX_UI_ZOOM, percent / 100));
      applyUiZoom(zoom);
    } catch {
      applyUiZoom(DEFAULT_UI_ZOOM);
    }
  }

  function adjustUiZoom(direction: 1 | -1) {
    const base = Number.isFinite(uiZoom) ? uiZoom : DEFAULT_UI_ZOOM;
    const next = Math.max(MIN_UI_ZOOM, Math.min(MAX_UI_ZOOM, Math.round((base + direction * UI_ZOOM_STEP) * 100) / 100));
    applyUiZoom(next);
    saveUiZoomPreference(next);
  }

  function resetUiZoom() {
    applyUiZoom(DEFAULT_UI_ZOOM);
    saveUiZoomPreference(DEFAULT_UI_ZOOM);
  }

  function loadSidebarWidthPreference() {
    try {
      const raw = localStorage.getItem("grafium.sidebar.width");
      if (!raw) return;
      const parsed = Number(raw);
      if (!Number.isFinite(parsed)) return;
      sidebarWidth = Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, parsed));
    } catch {
      // Ignore localStorage failures and keep defaults.
    }
  }

  function resetSidebarWidth() {
    sidebarWidth = DEFAULT_SIDEBAR_WIDTH;
    saveSidebarWidthPreference(sidebarWidth);
  }

  function saveSidebarWidthPreference(width: number) {
    try {
      localStorage.setItem("grafium.sidebar.width", String(Math.round(width)));
    } catch {
      // Ignore localStorage failures.
    }
  }

  function applySidebarWidthFromPointer(clientX: number) {
    if (!appLayoutEl) return;
    const rect = appLayoutEl.getBoundingClientRect();
    const maxByLayout = Math.max(SIDEBAR_MIN_WIDTH, rect.width - MAIN_CONTENT_MIN_WIDTH);
    const maxWidth = Math.min(SIDEBAR_MAX_WIDTH, maxByLayout);
    const next = clientX - rect.left;
    sidebarWidth = Math.max(SIDEBAR_MIN_WIDTH, Math.min(maxWidth, next));
  }

  function startSidebarResize(e: PointerEvent) {
    if (!sidebarVisible || zenMode || window.innerWidth <= 640) return;
    e.preventDefault();
    isResizingSidebar = true;
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";

    const onMove = (moveEvent: PointerEvent) => {
      applySidebarWidthFromPointer(moveEvent.clientX);
    };

    const onUp = () => {
      isResizingSidebar = false;
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
      saveSidebarWidthPreference(sidebarWidth);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  function logNav(event: string, data: unknown) {
    console.log(`[nav] ${event} ${JSON.stringify(data)}`);
  }

  function currentScrollTop(): number {
    return mainContentEl?.scrollTop ?? 0;
  }

  function getCurrentHistoryEntry(): HistoryEntry | null {
    if (currentView === "page" && currentPage) {
      return { kind: "page", title: currentPage.title, scrollTop: currentScrollTop() };
    }
    if (currentView === "journal") {
      return { kind: "journal", scrollTop: currentScrollTop() };
    }
    if (currentView === "all-pages") {
      return { kind: "all-pages", scrollTop: currentScrollTop() };
    }
    if (currentView === "graph") {
      return { kind: "graph", scrollTop: 0 };
    }
    if (currentView === "flashcards") {
      return { kind: "flashcards", scrollTop: currentScrollTop() };
    }
    if (currentView === "statistics") {
      return { kind: "statistics", scrollTop: currentScrollTop() };
    }
    if (currentView === "settings") {
      return { kind: "settings", scrollTop: currentScrollTop() };
    }
    if (currentView === "chat") {
      return { kind: "chat", scrollTop: currentScrollTop() };
    }
    return null;
  }

  function saveCurrentHistoryState(sourceBlockId?: string, sourcePageTitle?: string) {
    if (navIndex < 0 || navIndex >= navHistory.length) return;

    const current = getCurrentHistoryEntry();
    if (!current) return;

    navHistory = navHistory.map((entry, index) => {
      if (index !== navIndex) return entry;
      return {
        ...entry,
        ...current,
        sourceBlockId,
        sourcePageTitle,
      };
    });
  }

  function pushHistoryEntry(entry: HistoryEntry) {
    const nextHistory = [...navHistory.slice(0, navIndex + 1), entry];
    navHistory = nextHistory;
    navIndex = nextHistory.length - 1;
    logNav("push", { navIndex: nextHistory.length - 1, entry, historyLength: nextHistory.length });
  }

  function clearRestoreTimer() {
    if (restoreTimer !== null) {
      window.clearInterval(restoreTimer);
      restoreTimer = null;
    }
  }

  function restoreHistoryState(entry: HistoryEntry) {
    clearRestoreTimer();

    const startedAt = Date.now();
    let pageFallbackApplied = false;
    const tryRestore = () => {
      if (!mainContentEl) return false;

      if (entry.sourceBlockId) {
        const blockEl = mainContentEl.querySelector(`#block-${entry.sourceBlockId}, [data-block-id="${entry.sourceBlockId}"]`) as HTMLElement | null;
        if (blockEl) {
          blockEl.scrollIntoView({ block: "center" });
          logNav("restored block", { sourceBlockId: entry.sourceBlockId, sourcePageTitle: entry.sourcePageTitle, kind: entry.kind, title: entry.title });
          return true;
        }
        if (entry.kind === "page" && currentPage) {
          window.dispatchEvent(new CustomEvent("page-content-reveal-block", {
            detail: {
              pageId: currentPage.id,
              blockId: entry.sourceBlockId,
              align: "center",
            },
          }));
        }
      }

      if (entry.kind === "journal" && entry.sourcePageTitle) {
        const pageEl = mainContentEl.querySelector(`#journal-page-${CSS.escape(entry.sourcePageTitle)}, [data-page-title="${entry.sourcePageTitle}"]`) as HTMLElement | null;
        if (pageEl) {
          if (!pageFallbackApplied) {
            pageEl.scrollIntoView({ block: "center" });
            pageFallbackApplied = true;
            logNav("restored page fallback", { sourcePageTitle: entry.sourcePageTitle, sourceBlockId: entry.sourceBlockId, kind: entry.kind });
          }
          if (!entry.sourceBlockId) {
            return true;
          }
        }
      }

      if (!pageFallbackApplied) {
        mainContentEl.scrollTop = entry.scrollTop;
      }
      return !entry.sourceBlockId;
    };

    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (tryRestore()) {
          return;
        }

        restoreTimer = window.setInterval(() => {
          if (tryRestore()) {
            clearRestoreTimer();
            return;
          }

          if (Date.now() - startedAt > 3000) {
            logNav("restore timeout", { sourceBlockId: entry.sourceBlockId, sourcePageTitle: entry.sourcePageTitle, kind: entry.kind, title: entry.title, scrollTop: entry.scrollTop });
            clearRestoreTimer();
          }
        }, 75);
      });
    });
  }

  async function navigateToHistoryEntry(entry: HistoryEntry) {
    if (entry.kind === "journal") {
      await navigateToJournal(true, entry);
      return;
    }

    if (entry.kind === "all-pages") {
      currentView = "all-pages";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.kind === "graph") {
      currentView = "graph";
      loading = false;
      error = null;
      await tick();
      return;
    }

    if (entry.kind === "flashcards") {
      currentView = "flashcards";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.kind === "statistics") {
      currentView = "statistics";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.kind === "settings") {
      currentView = "settings";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.kind === "chat") {
      currentView = "chat";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.title) {
      await navigateToPage(entry.title, false, true, entry);
    }
  }

  async function goBack() {
    if (navIndex <= 0) return;
    saveCurrentHistoryState();
    navIndex -= 1;
    logNav("back", { navIndex, entry: navHistory[navIndex] });
    await navigateToHistoryEntry(navHistory[navIndex]);
  }

  async function goForward() {
    if (navIndex >= navHistory.length - 1) return;
    saveCurrentHistoryState();
    navIndex += 1;
    logNav("forward", { navIndex, entry: navHistory[navIndex] });
    await navigateToHistoryEntry(navHistory[navIndex]);
  }

  // Register hotkeys
  registerDefaultShortcuts({
    goJournal: () => navigateToJournal(),
    goHome: () => navigateToJournal(),
    goAllPages: () => navigateToPage("__all_pages__"),
    goGraph: () => navigateToPage("__graph__"),
    goFlashcards: () => navigateToPage("__flashcards__"),
    goTomorrow: () => {
      const d = new Date();
      d.setDate(d.getDate() + 1);
      navigateToPage(d.toISOString().split("T")[0], true);
    },
    goNextJournal: () => {
      if (currentPage && /^\d{4}-\d{2}-\d{2}$/.test(currentPage.title)) {
        const d = new Date(currentPage.title);
        d.setDate(d.getDate() + 1);
        navigateToPage(d.toISOString().split("T")[0], true);
      }
    },
    goPrevJournal: () => {
      if (currentPage && /^\d{4}-\d{2}-\d{2}$/.test(currentPage.title)) {
        const d = new Date(currentPage.title);
        d.setDate(d.getDate() - 1);
        navigateToPage(d.toISOString().split("T")[0], true);
      }
    },
    goForward: () => {
      goForward();
    },
    goBackward: () => {
      goBack();
    },
    search: () => {
      // Trigger sidebar search
      window.dispatchEvent(new CustomEvent("toggle-search"));
    },
    searchInPage: () => {
      window.dispatchEvent(new CustomEvent("toggle-search"));
    },
    toggleSidebar: () => {
      sidebarVisible = !sidebarVisible;
    },
    toggleRightSidebar: () => {
      referencePanelVisible = !referencePanelVisible;
    },
    toggleTheme: () => {}, // Not implemented yet
    toggleHelp: () => {
      window.dispatchEvent(new CustomEvent("toggle-help"));
    },
    toggleSettings: () => {
      navigateToPage("__settings__");
    },
    toggleWideMode: () => {},
    toggleZenMode: () => {
      zenMode = !zenMode;
    },
    newPage: () => {
      newPageName = "";
      showNewPageDialog = true;
    },
    reindex: () => {
      void runReindex(true);
    },
    undo: () => {},
    redo: () => {},
    commandPalette: () => {},
  });

  // Global keydown handler
  function handleGlobalKeydown(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey) {
      const key = e.key.toLowerCase();
      if (key === "0") {
        e.preventDefault();
        resetUiZoom();
        return;
      }
      if (key === "+" || key === "=" || (key === "-" && e.shiftKey)) {
        e.preventDefault();
        adjustUiZoom(1);
        return;
      }
      if (key === "-") {
        e.preventDefault();
        adjustUiZoom(-1);
        return;
      }
    }

    // Keep search shortcut global, including while editing.
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      window.dispatchEvent(new CustomEvent("toggle-search"));
      return;
    }

    // Ctrl+. toggles reference panel (always works regardless of editing state)
    if ((e.ctrlKey || e.metaKey) && e.key === ".") {
      e.preventDefault();
      referencePanelVisible = !referencePanelVisible;
      return;
    }

    // Escape closes reference panel (always works)
    if (e.key === "Escape" && referencePanelVisible) {
      referencePanelVisible = false;
      e.preventDefault();
      return;
    }

    // Don't intercept if a dialog is open
    if (showNewPageDialog) return;

    // Never hijack native editor/navigation keys while typing in editable elements.
    const target = e.target as HTMLElement | null;
    const editableContainer = target?.closest?.("[contenteditable='true'], [role='textbox']");
    const isNativeInput =
      target?.tagName === "INPUT" ||
      target?.tagName === "TEXTAREA" ||
      target?.tagName === "SELECT";
    if (isNativeInput || target?.isContentEditable || !!editableContainer) {
      return;
    }

    // Keep paging keys reliable regardless of lingering focus on editor DOM after Escape.
    if (mainContentEl && (e.key === "PageDown" || e.key === "PageUp" || e.key === "Home" || e.key === "End")) {
      e.preventDefault();
      if (e.key === "PageDown") {
        mainContentEl.scrollBy({ top: Math.max(120, mainContentEl.clientHeight * 0.9), behavior: "auto" });
      } else if (e.key === "PageUp") {
        mainContentEl.scrollBy({ top: -Math.max(120, mainContentEl.clientHeight * 0.9), behavior: "auto" });
      } else if (e.key === "Home") {
        mainContentEl.scrollTo({ top: 0, behavior: "auto" });
      } else if (e.key === "End") {
        mainContentEl.scrollTo({ top: mainContentEl.scrollHeight, behavior: "auto" });
      }
      return;
    }

    // Don't intercept if editing a block
    if (keymap_manager.isEditing) return;

    // Escape exits zen mode
    if (zenMode && e.key === "Escape") {
      zenMode = false;
      e.preventDefault();
      return;
    }

    keymap_manager.handleKeydown(e);
  }

  function handleMouseNavigation(e: MouseEvent) {
    if (e.button === 3) {
      e.preventDefault();
      goBack();
    } else if (e.button === 4) {
      e.preventDefault();
      goForward();
    }
  }

  function handleWheelZoom(e: WheelEvent) {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    const direction: 1 | -1 = e.deltaY < 0 ? 1 : -1;
    adjustUiZoom(direction);
  }

  $effect(() => {
    const detachUndoRedo = attachAppUndoRedoListeners();
    return () => {
      detachUndoRedo();
    };
  });

  $effect(() => {
    loadUiZoomPreference();
    window.addEventListener("keydown", handleGlobalKeydown, true);
    window.addEventListener("mouseup", handleMouseNavigation);
    window.addEventListener("wheel", handleWheelZoom, { passive: false });
    window.addEventListener("toggle-reference-panel", () => {
      referencePanelVisible = !referencePanelVisible;
    });
    return () => {
      window.removeEventListener("keydown", handleGlobalKeydown, true);
      window.removeEventListener("mouseup", handleMouseNavigation);
      window.removeEventListener("wheel", handleWheelZoom);
      clearRestoreTimer();
    };
  });

  // Navigate to tutorial welcome page on start (only once)
  let hasInitialized = false;
  $effect(() => {
    if (!hasInitialized) {
      hasInitialized = true;
      navigateToStartupPage();
      // Initialize theme
      initTheme();
    }
  });

  async function navigateToStartupPage() {
    try {
      // Only open Welcome when it already exists (tutorial graph).
      await getPage({ title: "Welcome To Grafium" });
      await navigateToPage("Welcome To Grafium");
      return;
    } catch {
      // Fallback for non-tutorial/custom graphs.
      await navigateToJournal();
    }
  }

  async function initTheme() {
    // Register listener first — must always succeed regardless of saved theme state
    listen<{ theme: string }>("smplos-theme-changed", (event) => {
      const t = getThemeById(event.payload.theme);
      if (t) {
        applyTheme(t.colors);
      }
    });

    // Apply saved/smplos theme on startup
    try {
      const [appTheme, smplosTheme] = await Promise.all([getAppTheme(), getSmplosTheme()]);
      const themeId = appTheme === "auto" ? (smplosTheme ?? "catppuccin") : appTheme;
      const theme = getThemeById(themeId);
      if (theme) {
        applyTheme(theme.colors);
      }
    } catch (e) {
      // If theme commands fail, fall back to smplos or default
      try {
        const smplos = await getSmplosTheme();
        const t = getThemeById(smplos ?? "catppuccin");
        if (t) applyTheme(t.colors);
      } catch (_) {}
    }
  }

  async function navigateToJournal(skipHistory = false, restoreEntry?: HistoryEntry) {
    if (!skipHistory) {
      saveCurrentHistoryState();
    }
    pendingJournalRestore = restoreEntry ?? null;
    if (restoreEntry) {
      journalRestoreRequestId += 1;
    }
    error = null;
    currentView = "journal";
    currentPage = null;
    loading = false;
    if (!skipHistory) {
      pushHistoryEntry({ kind: "journal", scrollTop: 0 });
    }
    await tick();
    if (restoreEntry) {
      restoreHistoryState(restoreEntry);
    }
  }

  async function navigateToPage(
    target: PageNavigationTarget,
    isJournal = false,
    skipHistory = false,
    restoreEntry?: HistoryEntry,
    sourceBlockId?: string,
    sourcePageTitle?: string
  ) {
    if (!skipHistory) {
      saveCurrentHistoryState(sourceBlockId, sourcePageTitle);
    }

    // Handle special routes
    if (target === "__all_pages__") {
      currentView = "all-pages";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "all-pages", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__graph__") {
      currentView = "graph";
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "graph", scrollTop: 0 });
      }
      await tick();
      return;
    }
    if (target === "__flashcards__") {
      currentView = "flashcards";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "flashcards", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__statistics__") {
      currentView = "statistics";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "statistics", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__settings__") {
      currentView = "settings";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "settings", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__chat__") {
      currentView = "chat";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "chat", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }

    const pageLookup = resolvePageLookup(target);

    loading = true;
    error = null;
    try {
      // Try to get existing page
      currentPage = await getPage(pageLookup);
    } catch (e) {
      // Create it if it doesn't exist
      if (!pageLookup.title) {
        error = `Failed to load page: ${e}`;
        loading = false;
        return;
      }
      try {
        currentPage = await createPage(
          pageLookup.title,
          isJournal || /^\d{4}-\d{2}-\d{2}$/.test(pageLookup.title)
        );
      } catch (e) {
        error = `Failed to load page: ${e}`;
        loading = false;
        return;
      }
    }

    if (currentPage) {
      recordPageOpen(currentPage.id).catch(() => {});
    }

    currentView = "page";
    loading = false;
    if (!skipHistory) {
      pushHistoryEntry({
        kind: "page",
        title: currentPage.title,
        scrollTop: 0,
        sourceBlockId,
        sourcePageTitle,
      });
    }
    await tick();
    if (restoreEntry) {
      restoreHistoryState(restoreEntry);
    } else if (mainContentEl) {
      mainContentEl.scrollTop = 0;
    }
  }

  let showNewPageDialog = $state(false);
  let newPageName = $state("");
  let showMoreMenu = $state(false);
  let showCreateGraphDialog = $state(false);
  let newGraphName = $state("");

  function handleNavigate(target: PageNavigationTarget) {
    if (target === "__journal__") {
      navigateToJournal();
      return;
    }
    if (target === "__new_page__") {
      newPageName = "";
      showNewPageDialog = true;
      return;
    }
    if (target === "__import_media__") {
      openImportMediaDialog();
      return;
    }
    navigateToPage(target);
  }

  function submitNewPage() {
    if (newPageName.trim()) {
      navigateToPage(newPageName.trim());
    }
    showNewPageDialog = false;
    newPageName = "";
  }

  function cancelNewPage() {
    showNewPageDialog = false;
    newPageName = "";
  }

  function handleNewPageKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") submitNewPage();
    if (e.key === "Escape") cancelNewPage();
  }

  // Import from media (video/audio -> transcript page): a URL or local file
  // path, transcribed via captions/Whisper (see `commands::media`) then
  // opened like any other page. Shares the same dialog styling as "New
  // Page" so it feels like one consistent affordance rather than a
  // separate feature bolted on.
  let showImportMediaDialog = $state(false);
  let importMediaUrl = $state("");
  let importMediaBusy = $state(false);
  let importMediaError = $state("");
  let importMediaProgress = $state("");

  function openImportMediaDialog() {
    importMediaUrl = "";
    importMediaError = "";
    importMediaProgress = "";
    importMediaBusy = false;
    showImportMediaDialog = true;
  }

  function cancelImportMedia() {
    if (importMediaBusy) return;
    showImportMediaDialog = false;
    importMediaUrl = "";
    importMediaError = "";
  }

  async function submitImportMedia() {
    const url = importMediaUrl.trim();
    if (!url || importMediaBusy) return;
    importMediaBusy = true;
    importMediaError = "";
    importMediaProgress = "Starting import...";
    const unlisten = await listen<string>("media-import-progress", (e) => {
      importMediaProgress = e.payload;
    });
    try {
      const page = await mediaImportVideo(url);
      showImportMediaDialog = false;
      importMediaUrl = "";
      navigateToPage(page.title);
    } catch (e) {
      importMediaError = e instanceof Error ? e.message : String(e);
    } finally {
      unlisten();
      importMediaBusy = false;
      importMediaProgress = "";
    }
  }

  function handleImportMediaKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") submitImportMedia();
    if (e.key === "Escape") cancelImportMedia();
  }

  function toggleMoreMenu() {
    showMoreMenu = !showMoreMenu;
  }

  function closeMoreMenu() {
    showMoreMenu = false;
  }

  async function runReindex(showSuccessAlert = false) {
    try {
      await reindexCurrent();
      handleGraphChanged();
      if (showSuccessAlert) {
        alert("Graph re-index complete.");
      }
    } catch (e) {
      console.error("[graph] reindex error:", e);
      alert("Re-index failed: " + e);
    }
  }

  async function handleMobileReindex() {
    closeMoreMenu();
    await runReindex(true);
  }

  async function handleMobileOpenGraph() {
    closeMoreMenu();
    console.log("[graph] opening folder picker for Open Graph");
    const selected = await pickFolder();
    console.log("[graph] pickFolder returned:", selected);
    if (selected) {
      try {
        const report = await validateGraph(selected);
        if (!report.is_valid) {
          const missing = [
            !report.has_pages_dir    && "pages/",
            !report.has_journals_dir && "journals/",
            !report.has_metadata_dir && "metadata/",
            !report.has_valid_db     && "metadata/index.db (corrupted)",
          ].filter(Boolean).join(", ");
          alert(
            `Not a valid Grafium graph.\n\n` +
            `Missing: ${missing}\n\n` +
            `Use New Graph to create a graph here instead.`
          );
          return;
        }
        await openGraph(selected);
        handleGraphChanged();
      } catch (e) {
        console.error("[graph] openGraph error:", e);
        alert("Failed to open graph: " + e);
      }
    }
  }

  function handleMobileCreateGraph() {
    closeMoreMenu();
    newGraphName = "";
    showCreateGraphDialog = true;
  }

  async function confirmCreateGraph() {
    if (!newGraphName.trim()) return;
    console.log("[graph] opening folder picker for Create Graph");
    showCreateGraphDialog = false;
    const selected = await pickFolder();
    console.log("[graph] pickFolder returned:", selected);
    if (selected) {
      const graphPath = selected + "/" + newGraphName.trim();
      try {
        await createGraph(graphPath, newGraphName.trim());
        handleGraphChanged();
      } catch (e) {
        console.error("[graph] createGraph error:", e);
        alert("Failed to create graph: " + e);
      }
    }
  }

  function cancelCreateGraph() {
    showCreateGraphDialog = false;
    newGraphName = "";
  }

  function handleGraphChanged() {
    // Reload after graph switch — bump request ID so JournalView's $effect re-fires
    journalRestoreRequestId += 1;
    navigateToJournal();
  }

  // Listen for page navigation events from rendered content
  function handlePageNav(e: Event) {
    const detail = (e as CustomEvent<string | LinkNavigateDetail>).detail;
    if (typeof detail === "string") {
      navigateToPage(detail);
      return;
    }

    if (detail.targetBlockId) {
      const restoreEntry: HistoryEntry = {
        kind: "page",
        title: detail.pageName,
        scrollTop: 0,
        sourceBlockId: detail.targetBlockId,
        sourcePageTitle: detail.sourcePageTitle ?? detail.pageName,
      };
      navigateToPage(
        detail.pageName,
        false,
        false,
        restoreEntry,
        detail.sourceBlockId,
        detail.sourcePageTitle
      );
      return;
    }

    navigateToPage(detail.pageName, false, false, undefined, detail.sourceBlockId, detail.sourcePageTitle);
  }

  $effect(() => {
    window.addEventListener("navigate-page", handlePageNav);
    return () => window.removeEventListener("navigate-page", handlePageNav);
  });

  $effect(() => {
    loadSidebarWidthPreference();
  });
</script>

<div class="app-shell" class:zen={zenMode}>
  {#if !zenMode}
    <TitleBar
      {sidebarVisible}
      {uiZoom}
      canGoBack={navIndex > 0}
      canGoForward={navIndex < navHistory.length - 1}
      onGoBack={goBack}
      onGoForward={goForward}
      onToggleReferencePanel={() => (referencePanelVisible = !referencePanelVisible)}
      onZoomIn={() => adjustUiZoom(1)}
      onZoomOut={() => adjustUiZoom(-1)}
      onZoomReset={resetUiZoom}
    />
  {/if}
  <div class="app-layout" bind:this={appLayoutEl}>
    {#if sidebarVisible && !zenMode}
      <div class="sidebar-container" style={`width: ${sidebarWidth}px;`}>
        <Sidebar {currentPage} {sidebarWidth} onNavigate={handleNavigate} onGraphChanged={handleGraphChanged} />
      </div>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="sidebar-resizer"
        class:resizing={isResizingSidebar}
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize sidebar"
        onpointerdown={startSidebarResize}
        ondblclick={resetSidebarWidth}
      ></div>
    {/if}

    <main bind:this={mainContentEl} class="main-content" class:zen-content={zenMode}>
    {#if error}
      <div class="error-state">
        <p>{error}</p>
        <button onclick={() => navigateToJournal()}>Retry</button>
      </div>
    {:else if loading}
      <div class="loading">Loading...</div>
    {:else if currentView === "all-pages"}
      <AllPages onNavigate={handleNavigate} />
    {:else if currentView === "graph"}
      <GraphView
        onNavigate={handleNavigate}
        currentPageId={currentPage?.id ?? ""}
        currentPageTitle={currentPage?.title ?? ""}
      />
    {:else if currentView === "statistics"}
      <Statistics onNavigate={handleNavigate} />
    {:else if currentView === "flashcards"}
      <FlashcardReview onNavigate={handleNavigate} />
    {:else if currentView === "chat"}
      <ChatbotView onOpenSettings={() => handleNavigate("__settings__")} />
    {:else if currentView === "settings"}
      <Settings />
    {:else if currentView === "journal"}
      <JournalView
        restorePageTitle={pendingJournalRestore?.sourcePageTitle}
        restoreRequestId={journalRestoreRequestId}
        onNavigate={handleNavigate}
      />
    {:else if currentView === "page" && currentPage}
      {#key currentPage.id}
        <PageContent page={currentPage} />
      {/key}
    {/if}
    </main>

    <!-- Reference / Knowledge Panel -->
    {#if referencePanelVisible}
      <div style="position:fixed;top:40px;right:0;bottom:0;width:380px;background:#1a1a2e;border-left:2px solid #e74c3c;z-index:9999;display:flex;flex-direction:column;padding:16px;color:#fff;">
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
          <h3 style="margin:0;">Knowledge Panel</h3>
          <button onclick={() => (referencePanelVisible = false)} style="background:none;border:none;color:#fff;font-size:20px;cursor:pointer;">✕</button>
        </div>
        <p style="color:#aaa;">Panel is working! Press Escape or click ✕ to close.</p>
        <ReferencePanel
          visible={true}
          pageId={currentPage?.id || ""}
          pageTitle={currentPage?.title || ""}
          onClose={() => (referencePanelVisible = false)}
          onNavigate={(target) => { referencePanelVisible = false; handleNavigate(target); }}
        />
      </div>
    {/if}

    <!-- Bottom nav for narrow screens -->
    <nav class="bottom-nav">
      <button class="bottom-nav-item" class:active={currentView === "journal"} onclick={() => handleNavigate("__journal__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="3" y="4" width="18" height="18" rx="2" ry="2"></rect>
          <line x1="16" y1="2" x2="16" y2="6"></line>
          <line x1="8" y1="2" x2="8" y2="6"></line>
          <line x1="3" y1="10" x2="21" y2="10"></line>
        </svg>
        <span>Journal</span>
      </button>
      <button class="bottom-nav-item" class:active={currentView === "statistics"} onclick={() => handleNavigate("__statistics__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M18 20V10"></path>
          <path d="M12 20V4"></path>
          <path d="M6 20v-6"></path>
        </svg>
        <span>Stats</span>
      </button>
      <button class="bottom-nav-item" class:active={currentView === "all-pages"} onclick={() => handleNavigate("__all_pages__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
          <polyline points="14 2 14 8 20 8"></polyline>
        </svg>
        <span>Pages</span>
      </button>
      <button class="bottom-nav-item" class:active={currentView === "graph"} onclick={() => handleNavigate("__graph__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="5" cy="6" r="2"></circle>
          <circle cx="19" cy="6" r="2"></circle>
          <circle cx="12" cy="18" r="2"></circle>
          <line x1="6.7" y1="7" x2="10.5" y2="16.3"></line>
          <line x1="17.3" y1="7" x2="13.5" y2="16.3"></line>
          <line x1="7" y1="6" x2="17" y2="6"></line>
        </svg>
        <span>Graph</span>
      </button>
      <button class="bottom-nav-item" class:active={currentView === "chat"} onclick={() => handleNavigate("__chat__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
        </svg>
        <span>Chat</span>
      </button>
      <button class="bottom-nav-item" onclick={() => handleNavigate("__new_page__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="12" y1="5" x2="12" y2="19"></line>
          <line x1="5" y1="12" x2="19" y2="12"></line>
        </svg>
        <span>New</span>
      </button>
      <button class="bottom-nav-item" class:active={referencePanelVisible} onclick={() => (referencePanelVisible = !referencePanelVisible)}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"></path>
          <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"></path>
        </svg>
        <span>AI</span>
      </button>
      <button class="bottom-nav-item" class:active={showMoreMenu || currentView === "settings"} onclick={toggleMoreMenu}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="5" r="1"></circle>
          <circle cx="12" cy="12" r="1"></circle>
          <circle cx="12" cy="19" r="1"></circle>
        </svg>
        <span>More</span>
      </button>
    </nav>

    <!-- More menu popup -->
    {#if showMoreMenu}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="more-menu-backdrop" onclick={closeMoreMenu}></div>
      <div class="more-menu">
        <button class="more-menu-item" onclick={() => { closeMoreMenu(); handleNavigate("__settings__"); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="3"></circle>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
          </svg>
          <span>Settings</span>
        </button>
        <button class="more-menu-item" onclick={() => { closeMoreMenu(); handleNavigate("__chat__"); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
          </svg>
          <span>Chatbot</span>
        </button>
        <button class="more-menu-item" onclick={handleMobileOpenGraph}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
          </svg>
          <span>Open Graph</span>
        </button>
        <button class="more-menu-item" onclick={() => { closeMoreMenu(); handleNavigate("__import_media__"); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polygon points="23 7 16 12 23 17 23 7"></polygon>
            <rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect>
          </svg>
          <span>Import Media</span>
        </button>
        <button class="more-menu-item" onclick={handleMobileCreateGraph}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line>
          </svg>
          <span>New Graph</span>
        </button>
        <button class="more-menu-item" onclick={handleMobileReindex}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polyline points="23 4 23 10 17 10"></polyline>
            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
          </svg>
          <span>Re-index Graph (Manual)</span>
        </button>
      </div>
    {/if}
  </div>
</div>

{#if showNewPageDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={cancelNewPage}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">New Page</h3>
      <input
        type="text"
        class="dialog-input"
        placeholder="Page name..."
        bind:value={newPageName}
        onkeydown={handleNewPageKeydown}
        autofocus
      />
      <div class="dialog-actions">
        <button class="dialog-btn dialog-btn-cancel" onclick={cancelNewPage}>Cancel</button>
        <button class="dialog-btn dialog-btn-ok" onclick={submitNewPage}>Create</button>
      </div>
    </div>
  </div>
{/if}

{#if showImportMediaDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={cancelImportMedia}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">Import from Video/Audio</h3>
      <p class="dialog-description">
        Paste a YouTube (or other yt-dlp-supported) URL, or a local file path. Captions are used
        if available; otherwise it falls back to local Whisper transcription if enabled in
        Settings.
      </p>
      <input
        type="text"
        class="dialog-input"
        placeholder="https://youtube.com/watch?v=... or /path/to/video.mp4"
        bind:value={importMediaUrl}
        onkeydown={handleImportMediaKeydown}
        disabled={importMediaBusy}
        autofocus
      />
      {#if importMediaBusy && importMediaProgress}
        <p class="dialog-progress">{importMediaProgress}</p>
      {/if}
      {#if importMediaError}
        <p class="dialog-error">{importMediaError}</p>
      {/if}
      <div class="dialog-actions">
        <button class="dialog-btn dialog-btn-cancel" onclick={cancelImportMedia} disabled={importMediaBusy}>Cancel</button>
        <button class="dialog-btn dialog-btn-ok" onclick={submitImportMedia} disabled={importMediaBusy || !importMediaUrl.trim()}>
          {importMediaBusy ? "Importing…" : "Import"}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if showCreateGraphDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={cancelCreateGraph}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">Create New Graph</h3>
      <p class="dialog-hint">Enter a name, then choose where to save it.</p>
      <input
        type="text"
        class="dialog-input"
        placeholder="Graph name..."
        bind:value={newGraphName}
        onkeydown={(e) => { if (e.key === "Enter") confirmCreateGraph(); if (e.key === "Escape") cancelCreateGraph(); }}
        autofocus
      />
      <div class="dialog-actions">
        <button class="dialog-btn dialog-btn-cancel" onclick={cancelCreateGraph}>Cancel</button>
        <button class="dialog-btn dialog-btn-ok" onclick={confirmCreateGraph} disabled={!newGraphName.trim()}>Choose Folder...</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .app-layout {
    display: flex;
    flex: 1;
    overflow: hidden;
    min-width: 0;
  }

  .sidebar-container {
    flex: 0 0 auto;
    min-width: 0;
    overflow: hidden;
  }

  .sidebar-container :global(.sidebar) {
    width: 100%;
    min-width: 0;
  }

  .sidebar-resizer {
    flex: 0 0 6px;
    cursor: col-resize;
    position: relative;
    background: transparent;
    border-left: 1px solid var(--border);
  }

  .sidebar-resizer::after {
    content: "";
    position: absolute;
    top: 0;
    left: 2px;
    width: 1px;
    height: 100%;
    background: color-mix(in srgb, var(--text-muted) 28%, transparent);
    opacity: 0;
    transition: opacity 0.12s ease;
  }

  .sidebar-resizer:hover::after,
  .sidebar-resizer.resizing::after {
    opacity: 1;
  }

  .main-content {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    background: var(--bg-primary);
    position: relative;
    min-width: 0;
    overflow-wrap: break-word;
    word-break: break-word;
    padding: 0;
    margin: 0;
  }

  .app-shell.zen {
    background: var(--bg-primary);
  }

  .main-content.zen-content {
    padding: 0;
  }

  .main-content.zen-content :global(.page-content) {
    max-width: 100%;
    padding: 2px 4px;
    margin: 0;
  }

  .main-content.zen-content :global(.journal-view) {
    max-width: 100%;
    padding: 2px 4px;
  }

  .main-content.zen-content :global(.backlinks-section) {
    display: none;
  }



  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: 16px;
  }

  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--danger);
    gap: 16px;
    padding: 24px;
    text-align: center;
  }

  .error-state button {
    padding: 8px 16px;
    background: var(--accent);
    color: white;
    border: none;
    border-radius: 6px;
    cursor: pointer;
  }

  .dialog-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }

  .dialog {
    background: var(--surface-overlay);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 24px;
    width: 360px;
    max-width: 90vw;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }

  .dialog-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 16px;
  }

  .dialog-hint {
    font-size: 12px;
    color: var(--text-muted);
    margin-bottom: 12px;
    font-family: monospace;
    word-break: break-all;
  }

  .dialog-description {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0 0 12px 0;
    line-height: 1.4;
  }

  .dialog-error {
    font-size: 12px;
    color: var(--error-color, #e57373);
    margin: 8px 0 0 0;
  }

  .dialog-progress {
    font-size: 12px;
    color: var(--text-secondary, #999);
    margin: 8px 0 0 0;
    font-style: italic;
  }

  .dialog-input {
    width: 100%;
    padding: 10px 12px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: 14px;
    outline: none;
    margin-bottom: 20px;
  }

  .dialog-input:focus {
    border-color: var(--accent);
  }

  .dialog-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .dialog-btn {
    padding: 8px 16px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border: none;
  }

  .dialog-btn-cancel {
    background: var(--btn-bg);
    color: var(--text-secondary);
  }

  .dialog-btn-cancel:hover {
    background: var(--btn-bg-hover);
    color: var(--text-primary);
  }

  .dialog-btn-ok {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
  }

  .dialog-btn-ok:hover {
    background: var(--btn-primary-hover);
  }

  .graph-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 12px;
    max-height: 200px;
    overflow-y: auto;
  }

  .graph-list-item {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition: background 0.1s;
  }

  .graph-list-item:hover {
    background: var(--bg-hover);
  }

  .graph-list-name {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .graph-list-path {
    font-size: 11px;
    color: var(--text-muted);
    font-family: monospace;
    word-break: break-all;
  }

  /* Bottom nav - hidden by default, shown on narrow screens */
  .bottom-nav {
    display: none;
  }

  @media (max-width: 640px) {
    .sidebar-resizer {
      display: none;
    }

    .bottom-nav {
      display: flex;
      position: fixed;
      bottom: 0;
      left: 0;
      right: 0;
      background: var(--bg-sidebar);
      border-top: 1px solid var(--border);
      padding: 4px 0;
      padding-bottom: env(safe-area-inset-bottom, 4px);
      z-index: 100;
      justify-content: space-around;
      align-items: center;
    }

    .bottom-nav-item {
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 2px;
      background: none;
      border: none;
      color: var(--text-muted);
      cursor: pointer;
      padding: 6px 12px;
      border-radius: 8px;
      font-size: 0.6rem;
      transition: color 0.15s;
    }

    .bottom-nav-item.active {
      color: var(--accent);
    }

    .bottom-nav-item:hover {
      color: var(--text-primary);
    }

    /* More menu popup */
    .more-menu-backdrop {
      position: fixed;
      top: 0;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 199;
    }

    .more-menu {
      position: fixed;
      bottom: 56px;
      right: 8px;
      background: var(--bg-secondary);
      border: 1px solid var(--border);
      border-radius: 12px;
      box-shadow: 0 -4px 24px rgba(0,0,0,0.4);
      z-index: 200;
      padding: 6px;
      min-width: 180px;
    }

    .more-menu-item {
      display: flex;
      align-items: center;
      gap: 10px;
      width: 100%;
      padding: 12px 14px;
      background: none;
      border: none;
      color: var(--text-primary);
      font-size: 14px;
      cursor: pointer;
      border-radius: 8px;
      transition: background 0.1s;
    }

    .more-menu-item:hover, .more-menu-item:active {
      background: var(--bg-hover);
    }

    /* Hide sidebar on narrow screens */
    :global(.sidebar) {
      display: none !important;
    }

    .main-content {
      padding-bottom: 60px;
    }
  }
</style>
